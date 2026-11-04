//! The one collector behind `stado cost report`, the run-time medians and
//! the autonomous control plane's attribution: every finished job with its
//! wall time, priced from the live quotes the coordinator keeps, and a job on
//! owned hardware from the energy its board drew.

use crate::queue::{JobStorage, StorageError};
use crate::scheduler::cost::measure::attribution::target_kind;
use crate::scheduler::cost::measure::timing::wall_seconds;

use super::row::CostRow;

#[path = "collect/measured.rs"]
pub(in crate::scheduler::cost) mod measured;

/// One row per finished job with its wall time. A cloud row is priced from
/// the stored price book; a row on owned hardware from the energy its board
/// drew in the hours it ran, as the host's beacon measured it, at the host's
/// declared USD tariff (`measured`). A job neither prices keeps its wall time
/// and has no rate or cost, so it is reported as unpriced rather than free,
/// and why is logged.
pub async fn collect_completed(store: &JobStorage) -> Result<Vec<CostRow>, StorageError> {
    let prices = crate::scheduler::scheduler::stored_price_book(store).await?;
    let mut finished = Vec::new();
    for state in [crate::queue::runs::COMPLETED, crate::queue::runs::FAILED] {
        for job in store.list_jobs(state).await? {
            finished.push((state, job));
        }
    }
    let owned = finished.iter().any(|(_, job)| {
        target_kind(job)
            .as_deref()
            .and_then(|kind| kind.parse().ok())
            == Some(crate::capabilities::ProviderId::Local)
    });
    // Read once, and only when an owned job needs it: a registry or beacon
    // store that cannot be read leaves owned jobs unpriced with that cause.
    let energy = match owned {
        true => Some(measured::Energy::read(store).await),
        false => None,
    };
    let jobs: Vec<crate::models::Job> = finished.iter().map(|(_, job)| job.clone()).collect();
    let mut rows = Vec::new();
    for (state, job) in &finished {
        // A job with no start or finish has no wall time to price; one
        // whose times cannot be read is left out and said, like a target
        // nobody prices below, so a total is never short without a word.
        let wall = match wall_seconds(job) {
            Ok(Some(wall)) => wall,
            Ok(None) => continue,
            Err(reason) => {
                tracing::warn!(
                    "job {} has no wall time to price, so it is left out of cost: {reason}",
                    job.job_id
                );
                continue;
            }
        };
        let target = target_kind(job);
        // A target that names no provider Stado prices leaves the job
        // unpriced, and says which target it was.
        let provider: Option<crate::capabilities::ProviderId> = match target
            .as_deref()
            .map(str::parse::<crate::capabilities::ProviderId>)
        {
            None => None,
            Some(Ok(provider)) => Some(provider),
            Some(Err(unknown)) => {
                tracing::warn!(
                    "job {} ran on target {target:?}, which names no provider Stado prices, so \
                     it is unpriced: {unknown}",
                    job.job_id
                );
                None
            }
        };
        let hours = wall / crate::monitor::billing::SECONDS_PER_HOUR as f64;
        let (rate, cost) = match provider {
            Some(crate::capabilities::ProviderId::Local) => {
                let priced = match &energy {
                    Some(Ok(energy)) => energy.price(job, &jobs),
                    Some(Err(cause)) => Err(cause.clone()),
                    None => Err("no owned job was expected".to_string()),
                };
                match priced {
                    Ok(cost) => ((hours > 0.0).then(|| cost / hours), Some(cost)),
                    Err(cause) => {
                        tracing::warn!(
                            "job {} ran on owned hardware and is unpriced: {cause}",
                            job.job_id
                        );
                        (None, None)
                    }
                }
            }
            Some(provider) => {
                let rate = prices
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
                    .map(|quote| quote.hourly_usd);
                (rate, rate.map(|rate| hours * rate))
            }
            None => (None, None),
        };
        rows.push(CostRow {
            job_id: job.job_id.clone(),
            state: (*state).into(),
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
            model: crate::sizing::model_of(&job.command),
        });
    }
    Ok(rows)
}
