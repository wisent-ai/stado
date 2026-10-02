//! Queue every declared delivery of one published run, then collect the
//! verdict each one reached. A delivery that names others in `after` is
//! queued only once each of them has passed.

use std::collections::BTreeMap;

use crate::cli::release_submit::builds::jobs::terminal::{ended, job_output_tail, Ended};
use crate::cli::release_submit::deliver::queue::{queue_delivery, record_unqueued};
use crate::cli::release_submit::run::state::save;
use crate::cli::CmdError;
use crate::models::job_state;
use crate::queue::storage::JobStorage;
use crate::release_control::{self, ReleaseArtifactRef};
use crate::release_pipeline::{Delivery, DeliveryRunState, ReleasePipelineManifest, ReleaseRun};

/// What one delivery pass found.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Deliveries {
    /// Every delivery has its verdict and no required one failed.
    Complete,
    /// At least one delivery is still queued or running on its host; the run
    /// stays delivering and the next pass reads it again.
    Pending,
}

pub(crate) async fn run_deliveries(
    run: &mut ReleaseRun,
    m: &ReleasePipelineManifest,
    artifacts: &BTreeMap<String, ReleaseArtifactRef>,
    deliveries: &[Delivery],
) -> Result<Deliveries, CmdError> {
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
    let mut pending = false;
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
        let job = match ended(&store, &current.job_id).await? {
            Ended::Job(job) => job,
            // Queued on its host or running there: nothing to judge yet. The
            // pass that ends a delivery is the one that finds its record or
            // receipt; this one records what it saw and leaves the run
            // delivering. Failing the run here made the delivery worker
            // refuse its own job a moment later ("the run is Failed, not
            // delivering") and no host received the release.
            Ended::Queued { state, host } => {
                let updated = run.deliveries.get_mut(&d.name).unwrap();
                updated.failure = Some(format!("queued ({state}) on {host}, not yet claimed"));
                pending = true;
                continue;
            }
            Ended::Running => {
                let updated = run.deliveries.get_mut(&d.name).unwrap();
                updated.failure = Some("running".to_string());
                pending = true;
                continue;
            }
        };
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
        None if pending => Ok(Deliveries::Pending),
        None => Ok(Deliveries::Complete),
    }
}
