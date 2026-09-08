//! Validators for the immutable snapshots a retention pass reads back: the
//! durable transition document and the queued cancellation request marker.

use crate::queue::storage::TransitionSnapshotProof;
use crate::queue::StorageError;

use super::{transition_path, JobTransition, TRANSITION_RETIRED_STATE, TRANSITION_SCHEMA};

/// Parse one immutable transition snapshot through the production transition
/// type and prove that its object key is the canonical digest key for its job.
pub(crate) fn validate_transition_snapshot(
    path: &str,
    content: &str,
) -> Result<TransitionSnapshotProof, StorageError> {
    let transition: JobTransition = serde_json::from_str(content)?;
    if transition.schema != TRANSITION_SCHEMA || path != transition_path(&transition.job_id) {
        return Err(StorageError::Other(format!(
            "invalid durable transition snapshot {path}"
        )));
    }
    Ok(TransitionSnapshotProof {
        job_id: transition.job_id,
        retired: transition.state == TRANSITION_RETIRED_STATE,
    })
}

pub(crate) fn validate_cancellation_snapshot(
    job_id: &str,
    content: &str,
) -> Result<(), StorageError> {
    let request: serde_json::Value = serde_json::from_str(content)?;
    if request.get("job_id").and_then(serde_json::Value::as_str) != Some(job_id) {
        return Err(StorageError::Other(format!(
            "cancellation marker does not belong to {job_id}"
        )));
    }
    let requested_at = request
        .get("requested_at")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| {
            StorageError::Other(format!(
                "cancellation marker for {job_id} has no requested_at"
            ))
        })?;
    chrono::DateTime::parse_from_rfc3339(requested_at).map_err(|error| {
        StorageError::Other(format!(
            "cancellation marker for {job_id} has invalid requested_at: {error}"
        ))
    })?;
    Ok(())
}
