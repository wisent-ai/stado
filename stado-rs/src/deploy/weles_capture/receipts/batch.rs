//! What one batch looks like now: the worker's action log, the objects the
//! store already holds, and the tally the report prints.

use serde_json::{json, Value};

use super::super::channel::Channel;
use super::super::{safe_component, ARTIFACT_NAMESPACE, CAPTURE_ACTION, QUERY_LIMIT, QUERY_ROUTE};
use super::{capture_state, BatchStatus, CaptureState};
use super::{STATE_DONE, STATE_FAILED, STATE_QUEUED, STATE_RUNNING};
use crate::deploy::DeployError;

/// Per-action state of one batch, plus the artifact keys already stored.
///
/// One action-log query and one storage listing. Rows are matched to the batch
/// by the `batch` param each action carries, and artifacts to the action by
/// the `artifact_prefix` it carries, so this reads correctly from a machine
/// that never ran the enqueue.
pub async fn status(channel: &Channel, batch: &str) -> Result<BatchStatus, DeployError> {
    safe_component("capture batch id", batch)?;
    let data = channel
        .call(
            QUERY_ROUTE,
            &json!({ "action": CAPTURE_ACTION, "limit": QUERY_LIMIT }),
        )
        .await?;
    let logs = data.get("logs").and_then(Value::as_array).ok_or_else(|| {
        DeployError(
            "the Weles admission API returned no action log for the capture action".to_string(),
        )
    })?;
    // A store that will not answer is not a batch with no artifacts. The
    // action states are still worth having, so the failure is carried beside
    // them instead of replacing them.
    let (artifacts, artifacts_unreachable) = match batch_artifacts(batch).await {
        Ok(artifacts) => (artifacts, None),
        Err(error) => (Vec::new(), Some(error.to_string())),
    };
    let mut states: Vec<CaptureState> = logs
        .iter()
        .filter(|row| row.pointer("/params/batch").and_then(Value::as_str) == Some(batch))
        .map(|row| {
            let artifact_prefix = row
                .pointer("/params/artifact_prefix")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            let owned = if artifact_prefix.is_empty() {
                Vec::new()
            } else {
                artifacts
                    .iter()
                    .filter(|uri| uri.starts_with(&artifact_prefix))
                    .cloned()
                    .collect()
            };
            CaptureState {
                action_id: row
                    .get("id")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                site_slug: row
                    .pointer("/params/site_slug")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                axis: row
                    .pointer("/params/axis")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                state: capture_state(
                    row.get("status")
                        .and_then(Value::as_str)
                        .unwrap_or_default(),
                ),
                error: row
                    .get("error")
                    .and_then(Value::as_str)
                    .map(str::to_string)
                    .filter(|error| !error.trim().is_empty()),
                artifact_prefix,
                artifacts: owned,
            }
        })
        .collect();
    // The action log is ordered by start time, which puts every capture that
    // has not started yet in one undifferentiated block. Site and axis are
    // what an operator scans for, so that is the order the report prints in.
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
/// them, plus any state word the worker used that this command does not know.
pub fn totals(states: &[CaptureState]) -> Vec<(String, usize)> {
    let mut totals: Vec<(String, usize)> = [STATE_QUEUED, STATE_RUNNING, STATE_DONE, STATE_FAILED]
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
