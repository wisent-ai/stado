//! The one collector behind `stado cost report`, the run-time medians and
//! the autonomous control plane's attribution: every finished job with its
//! wall time, priced from the live quotes the coordinator keeps.

use crate::queue::{JobStorage, StorageError};
use crate::scheduler::cost::measure::attribution::{model_from_command, target_kind};
use crate::scheduler::cost::measure::timing::wall_seconds;

use super::row::CostRow;

/// One row per finished job with its wall time. A cloud row is priced from
/// the stored price book; a row on owned hardware at the autonomy policy's
/// declared `local_hourly_cost_usd`. A job neither prices keeps its wall time
/// and has no rate or cost, so it is reported as unpriced rather than free.
pub async fn collect_completed(store: &JobStorage) -> Result<Vec<CostRow>, StorageError> {
    let prices = crate::scheduler::scheduler::stored_price_book(store).await?;
    let local_rate = crate::autonomy::storage::load_policy(store)
        .await?
        .and_then(|policy| policy.local_hourly_cost_usd);
    let mut rows = Vec::new();
    for state in ["completed", "failed"] {
        for job in store.list_jobs(state, 0).await? {
            let Some(wall) = wall_seconds(&job) else {
                continue;
            };
            let target = target_kind(&job);
            let provider: Option<crate::capabilities::ProviderId> = target.parse().ok();
            let rate = match provider {
                Some(crate::capabilities::ProviderId::Local) => local_rate,
                Some(provider) => prices
                    .as_ref()
                    .and_then(|book| {
                        book.find_hourly(
                            provider,
                            Some(job.region.as_str()).filter(|region| !region.is_empty()),
                            &job.machine_type,
                            &job.gpu_type,
                            job.preemptible,
                        )
                    })
                    .map(|quote| quote.hourly_usd),
                None => None,
            };
            let cost = rate.map(|rate| wall / crate::monitor::billing::SECONDS_PER_HOUR as f64 * rate);
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
