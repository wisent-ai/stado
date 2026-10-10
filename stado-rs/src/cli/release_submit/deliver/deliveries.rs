//! Queue every declared delivery of one published run, then collect the
//! verdict each one reached. A delivery that names others in `after` is
//! queued only once each of them has passed.

use std::collections::BTreeMap;

use crate::cli::release_submit::builds::jobs::terminal::{ended, job_output_tail, Ended};
use crate::cli::release_submit::deliver::queue::{
    delivery_request, queue_delivery, record_unqueued,
};
use crate::cli::release_submit::deliver::verified_receipt;
use crate::cli::release_submit::run::state::save;
use crate::cli::CmdError;
use crate::models::job_state;
use crate::queue::storage::JobStorage;
use crate::release_control::ReleaseArtifactRef;
use crate::release_pipeline::{Delivery, DeliveryRunState, ReleasePipelineManifest, ReleaseRun};

/// The submission scope every delivery attempt is queued under.
pub(super) const DELIVERY_RUN_SCOPE: &str = "release-delivery";

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
        // A leg recorded passed is not taken on that record: its job's
        // receipt is read and checked again below, before the deliveries
        // that come after it are admitted, so a receipt that went missing or
        // changed since it was accepted no longer passes it.
        let dependency_failure = d
            .after
            .iter()
            .find(|prior| !passed(prior))
            .map(|prior| format!("delivery {prior} did not pass"));
        if let Some(failure) = &dependency_failure {
            let has_job = run
                .deliveries
                .get(&d.name)
                .is_some_and(|current| !current.job_id.is_empty());
            if !has_job {
                record_unqueued(run, d, format!("not queued: {failure}"));
            }
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
        let with_dependency = |cause: String| match &dependency_failure {
            Some(dependency) => format!("{cause}; {dependency}"),
            None => cause,
        };
        let job = match ended(&store, &current.job_id).await? {
            Ended::Job(job) => *job,
            // Queued on its host or running there: nothing to judge yet. The
            // pass that ends a delivery is the one that finds its record or
            // receipt; this one records what it saw and leaves the run
            // delivering. Failing the run here made the delivery worker
            // refuse its own job a moment later ("the run is Failed, not
            // delivering") and no host received the release.
            Ended::Queued { state, host } => {
                let updated = run.deliveries.get_mut(&d.name).unwrap();
                updated.state = DeliveryRunState::Submitted;
                updated.failure = Some(with_dependency(format!(
                    "queued ({state}) on {host}, not yet claimed"
                )));
                pending = true;
                continue;
            }
            Ended::Running => {
                let updated = run.deliveries.get_mut(&d.name).unwrap();
                updated.state = DeliveryRunState::Submitted;
                updated.failure = Some(with_dependency("running".to_string()));
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
            .await;
        let verdict = match (ok, artifacts.get(&d.platform)) {
            (false, _) => Err(format!(
                "{}{}",
                match &job.error {
                    Some(error) => error.clone(),
                    None => job.state.clone(),
                },
                job_output_tail(&store, &current.job_id).await
            )),
            (true, None) => Err(format!(
                "no published artifact for delivery platform {} to check its receipt against",
                d.platform
            )),
            (true, Some(a)) => receipt.as_ref()
                .map_err(|cause| format!("cannot read delivery-receipt.json for job {}: {cause}", current.job_id))
                .and_then(|bytes| verified_receipt(bytes.as_deref(), &delivery_request(run, d, a), &current.job_id))
                .and_then(|sha256| match &current.receipt_sha256 {
                    Some(accepted) if *accepted != sha256 => Err(format!(
                        "the delivery-receipt.json of job {} hashes to {sha256}, but the run accepted {accepted}",
                        current.job_id
                    )),
                    _ => Ok(sha256),
                }),
        };
        let verdict = verdict.map_err(with_dependency).and_then(|sha256| {
            run.deliveries.get_mut(&d.name).unwrap().receipt_sha256 = Some(sha256);
            match dependency_failure {
                Some(cause) => Err(cause),
                None => Ok(()),
            }
        });
        let (state, failure) = match verdict {
            Ok(_) => (DeliveryRunState::Passed, None),
            Err(cause) => {
                if d.required && required_failure.is_none() {
                    // Collect all required delivery failures before returning.
                    required_failure =
                        Some(format!("required delivery {} failed: {cause}", d.name));
                }
                (DeliveryRunState::Failed, Some(cause))
            }
        };
        let updated = run.deliveries.get_mut(&d.name).unwrap();
        updated.state = state;
        updated.failure = failure;
    }
    save(run).await?;
    // The run is judged once every delivery has its verdict. Failing it while
    // a sibling is still queued on its host made that sibling's worker refuse
    // its own job ("the run is Failed, not delivering"), so one host's failure
    // withheld the release from every other host. The failed delivery's
    // verdict is already on the run; the pass that finds the last one ended
    // reports it.
    match required_failure {
        _ if pending => Ok(Deliveries::Pending),
        Some(failure) => {
            Err(CmdError::click(failure)
                .stating(crate::primitives::failure::FailureCode::InfraDown))
        }
        None => Ok(Deliveries::Complete),
    }
}
