//! Queue every declared delivery of one published run, then collect the
//! verdict each one reached. A delivery that names others in `after` is
//! queued only once each of them has passed.

use std::collections::BTreeMap;

use crate::cli::release_submit::builds::jobs::terminal::{job_output_tail, terminal};
use crate::cli::release_submit::deliver::queue::{queue_delivery, record_unqueued};
use crate::cli::release_submit::run::state::save;
use crate::cli::CmdError;
use crate::models::job_state;
use crate::queue::storage::JobStorage;
use crate::release_control::{self, ReleaseArtifactRef};
use crate::release_pipeline::{Delivery, DeliveryRunState, ReleasePipelineManifest, ReleaseRun};

pub(crate) async fn run_deliveries(
    run: &mut ReleaseRun,
    m: &ReleasePipelineManifest,
    artifacts: &BTreeMap<String, ReleaseArtifactRef>,
    deliveries: &[Delivery],
) -> Result<(), CmdError> {
    let store = JobStorage::new().await?;
    for d in deliveries.iter().filter(|d| d.after.is_empty()) {
        queue_delivery(run, m, artifacts, &store, d).await?;
    }

    // Queue every independent target before waiting for any one of them. A
    // silent host must not prevent later targets from receiving the same
    // immutable release: they are independent deliveries, even though their
    // required verdicts are collected into one release result. A delivery
    // with `after` is queued here, in declaration order, once the verdicts
    // it names are in: a schema delivery that failed must not be followed by
    // the application that reads that schema.
    let mut required_failure = None;
    for d in deliveries {
        let passed = |name: &str| {
            run.deliveries
                .get(name)
                .is_some_and(|prior| prior.state == DeliveryRunState::Passed)
        };
        if passed(&d.name) {
            continue;
        }
        if let Some(prior) = d.after.iter().find(|prior| !passed(prior)) {
            let failure = format!("not queued: delivery {prior} did not pass");
            record_unqueued(run, d, failure);
        } else if !d.after.is_empty() {
            queue_delivery(run, m, artifacts, &store, d).await?;
        }
        let current = run.deliveries[&d.name].clone();
        if current.job_id.is_empty() {
            if d.required && required_failure.is_none() {
                required_failure = Some(format!(
                    "required delivery {} failed: {}",
                    d.name,
                    current.failure.clone().unwrap_or_default()
                ));
            }
            continue;
        }
        let job = terminal(&store, &current.job_id).await?;
        let ok = matches!(
            job.state.as_str(),
            job_state::COMPLETED | job_state::UPLOADED
        );
        let receipt = store
            .read_bytes(&format!(
                "status/{}/output/delivery-receipt.json",
                current.job_id
            ))
            .await?;
        let updated = run.deliveries.get_mut(&d.name).unwrap();
        updated.receipt_sha256 = receipt.as_deref().map(release_control::sha256_bytes);
        updated.state = if ok {
            DeliveryRunState::Passed
        } else {
            DeliveryRunState::Failed
        };
        updated.failure = if ok {
            None
        } else {
            Some(format!(
                "{}{}",
                job.error.clone().unwrap_or_else(|| job.state.clone()),
                job_output_tail(&store, &current.job_id).await
            ))
        };
        if d.required && !ok && required_failure.is_none() {
            // Collect all required delivery failures before returning.
            let cause = updated.failure.clone().unwrap_or_else(|| job.state.clone());
            required_failure = Some(format!("required delivery {} failed: {cause}", d.name));
        }
    }
    save(run).await?;
    match required_failure {
        Some(failure) => Err(CmdError::click(failure)),
        None => Ok(()),
    }
}
