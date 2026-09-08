//! The two collectors: the catalog-priced parity sweep behind
//! `stado cost report`, and the dynamic-price attribution the autonomous
//! control plane bills from.

use crate::queue::{JobStorage, StorageError};
use crate::scheduler::cost::measure::attribution::{model_from_command, target_kind};
use crate::scheduler::cost::measure::rates::hourly_rate_usd;
use crate::scheduler::cost::measure::timing::wall_seconds;

use super::row::CostRow;

/// One entry per finished job with wall-time + cost attribution.
/// Python `collect_completed`.
pub async fn collect_completed(store: &JobStorage) -> Result<Vec<CostRow>, StorageError> {
    let mut rows = Vec::new();
    for state in ["completed", "failed"] {
        for job in store.list_jobs(state, 0).await? {
            let Some(wall) = wall_seconds(&job) else {
                continue;
            };
            let rate = hourly_rate_usd(&job.gpu_type, job.preemptible, &job.machine_type);
            let cost = (wall / 3600.0) * rate;
            rows.push(CostRow {
                job_id: job.job_id.clone(),
                state: state.into(),
                gpu_type: if job.gpu_type.is_empty() {
                    "cpu".into()
                } else {
                    job.gpu_type.clone()
                },
                preemptible: job.preemptible,
                wall_s: wall,
                rate_usd_hr: rate,
                cost_usd: cost,
                target_kind: target_kind(&job),
                model: model_from_command(&job.command),
            });
        }
    }
    Ok(rows)
}

/// Dynamic-price attribution for the autonomous control plane. Unlike the
/// legacy parity report, this never substitutes catalog constants for a live
/// cloud quote: an unpriced cloud row remains unmeasured.
pub async fn collect_completed_dynamic(store: &JobStorage) -> Result<Vec<CostRow>, StorageError> {
    let Some(prices) = crate::autonomy::storage::read_json::<crate::autonomy::cost::PriceBook>(
        store,
        "state/autonomy/cost/prices.json",
    )
    .await?
    else {
        return Ok(Vec::new());
    };
    let mut rows = Vec::new();
    for state in ["completed", "failed"] {
        for job in store.list_jobs(state, usize::default()).await? {
            let Some(wall) = wall_seconds(&job) else {
                continue;
            };
            let target = target_kind(&job);
            let provider = match target.as_str() {
                "gcp" => crate::capabilities::ProviderId::Gcp,
                "aws" => crate::capabilities::ProviderId::Aws,
                "azure" => crate::capabilities::ProviderId::Azure,
                "local" => crate::capabilities::ProviderId::Local,
                "box" => crate::capabilities::ProviderId::Box,
                "vast" => crate::capabilities::ProviderId::Vast,
                _ => continue,
            };
            let rate = if provider == crate::capabilities::ProviderId::Local {
                f64::default()
            } else {
                let Some(quote) = prices.find_hourly(
                    provider,
                    Some(job.region.as_str()).filter(|region| !region.is_empty()),
                    &job.machine_type,
                    &job.gpu_type,
                    job.preemptible,
                ) else {
                    continue;
                };
                quote.hourly_usd
            };
            let cost = wall / crate::monitor::billing::SECONDS_PER_HOUR as f64 * rate;
            rows.push(CostRow {
                job_id: job.job_id.clone(),
                state: state.into(),
                gpu_type: if job.gpu_type.is_empty() {
                    "cpu".into()
                } else {
                    job.gpu_type.clone()
                },
                preemptible: job.preemptible,
                wall_s: wall,
                rate_usd_hr: rate,
                cost_usd: cost,
                target_kind: target,
                model: model_from_command(&job.command),
            });
        }
    }
    Ok(rows)
}
