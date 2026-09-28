//! What one batch looks like now: Stado's record of the runs it started, the
//! objects the store already holds, and the tally the report prints.

use super::super::{safe_component, ARTIFACT_NAMESPACE};
use super::{record, BatchStatus, CaptureState};
use super::{STATE_DONE, STATE_FAILED};
use crate::deploy::DeployError;

/// Per-capture state of one batch, plus the artifact keys already stored.
///
/// One record read and one storage listing, both in Stado storage, so this
/// reads the same from any machine and never asks the Weles host. Artifacts
/// are matched to a capture by the `artifact_prefix` it was run with.
pub async fn status(batch: &str) -> Result<BatchStatus, DeployError> {
    safe_component("capture batch id", batch)?;
    let receipts = record::read(batch).await?;
    // A store that will not answer is not a batch with no artifacts. The
    // capture states are still worth having, so the failure is carried beside
    // them instead of replacing them.
    let (artifacts, artifacts_unreachable) = match batch_artifacts(batch).await {
        Ok(artifacts) => (artifacts, None),
        Err(error) => (Vec::new(), Some(error.to_string())),
    };
    let mut states: Vec<CaptureState> = receipts
        .into_iter()
        .map(|receipt| {
            let owned = if receipt.artifact_prefix.is_empty() {
                Vec::new()
            } else {
                artifacts
                    .iter()
                    .filter(|uri| uri.starts_with(&receipt.artifact_prefix))
                    .cloned()
                    .collect()
            };
            CaptureState {
                action_id: receipt.run_id,
                site_slug: receipt.site_slug,
                axis: receipt.axis,
                state: receipt.state,
                error: receipt.error.filter(|error| !error.trim().is_empty()),
                artifact_prefix: receipt.artifact_prefix,
                artifacts: owned,
            }
        })
        .collect();
    // Site and axis are what an operator scans for, so that is the order the
    // report prints in.
    states.sort_by(|left, right| {
        (&left.site_slug, &left.axis, &left.action_id).cmp(&(
            &right.site_slug,
            &right.axis,
            &right.action_id,
        ))
    });
    Ok(BatchStatus {
        captures: states,
        artifacts_unreachable,
    })
}

/// Every capture object already in Stado storage under one batch, through the
/// same provider-neutral surface `stado storage objects` and `storage get`
/// read. One listing serves the whole batch.
async fn batch_artifacts(batch: &str) -> Result<Vec<String>, DeployError> {
    crate::cli::storage::list_object_uris(ARTIFACT_NAMESPACE, &format!("{batch}/"))
        .await
        .map_err(|error| {
            DeployError(format!(
                "cannot list stado://{ARTIFACT_NAMESPACE}/{batch}/: {error}"
            ))
        })
}

/// How many captures sit in each state, in the fixed order the report prints
/// them, plus any state word a record carries that this command does not know.
pub fn totals(states: &[CaptureState]) -> Vec<(String, usize)> {
    let mut totals: Vec<(String, usize)> = [STATE_DONE, STATE_FAILED]
        .iter()
        .map(|state| ((*state).to_string(), usize::default()))
        .collect();
    for state in states {
        match totals.iter_mut().find(|(name, _)| name == &state.state) {
            Some((_, count)) => *count += 1,
            None => totals.push((state.state.clone(), 1)),
        }
    }
    totals
}
