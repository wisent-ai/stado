//! Run manifests and the canonical transition companions of their jobs: the
//! two families whose fate the run's retained terminal outcomes decide.

use serde_json::Value;

use crate::monitor::reap::manifest::has_complete_retained_outcomes;
use crate::monitor::reap::reads::required_snapshot_text;
use crate::queue::{JobStorage, StorageError};

use super::{LifecyclePath, SnapshotIndex};

pub(super) async fn classify_run_manifest(
    store: &JobStorage,
    index: &SnapshotIndex,
    decisions: &mut Vec<Value>,
    path: LifecyclePath<'_>,
) -> Result<(), StorageError> {
    let SnapshotIndex { run_documents, .. } = index;
    let LifecyclePath {
        tail, full_path, ..
    } = path;
    let Some(run_id) = tail.strip_suffix(".json") else {
        decisions.push(serde_json::json!({
            "kind": "block_unclassified_live",
            "path": full_path,
            "reason": "run manifest key is not canonical",
        }));
        return Ok(());
    };
    let document = run_documents
        .get(run_id)
        .ok_or_else(|| StorageError::NotFound(format!("runs/{run_id}.json")))?;
    if document.get("schema").and_then(Value::as_str) != Some("stado.run-submission.v3") {
        decisions.push(serde_json::json!({
            "kind": "preserve_historical_run",
            "path": full_path,
            "reason": "pre-v3 run history is retained without destructive validation",
        }));
    } else if has_complete_retained_outcomes(document) {
        decisions.push(serde_json::json!({
            "kind": "preserve_historical_run",
            "path": full_path,
            "reason": "strict retained terminal outcomes",
        }));
    } else {
        let status = crate::queue::runs::run_status(store, run_id)
            .await?
            .ok_or_else(|| StorageError::NotFound(format!("runs/{run_id}.json")))?;
        decisions.push(if status.all_terminal {
            serde_json::json!({
                "kind": "terminal_run_recovery",
                "path": full_path,
                "run_id": run_id,
            })
        } else {
            serde_json::json!({
                "kind": "block_unclassified_live",
                "path": full_path,
                "run_id": run_id,
                "reason": "validated v3 run still has live or missing work",
            })
        });
    }
    Ok(())
}

pub(super) async fn classify_transition_companion(
    store: &JobStorage,
    index: &SnapshotIndex,
    decisions: &mut Vec<Value>,
    path: LifecyclePath<'_>,
) -> Result<(), StorageError> {
    let SnapshotIndex { retained_jobs, .. } = index;
    let LifecyclePath {
        relative,
        full_path,
        ..
    } = path;
    let raw = required_snapshot_text(store, relative).await?;
    let transition = crate::queue::storage::validate_transition_snapshot(relative, &raw)?;
    if !transition.retired {
        decisions.push(serde_json::json!({
            "kind": "block_unclassified_live",
            "path": full_path,
            "job_id": transition.job_id,
            "reason": "canonical transition is not retired",
        }));
    } else if let Some(run_id) = retained_jobs.get(&transition.job_id) {
        decisions.push(serde_json::json!({
            "kind": "retained_outcome_cleanup",
            "run_id": run_id,
            "job_id": transition.job_id,
            "primary_only_paths": [],
            "transition_companions": [full_path],
        }));
    } else {
        decisions.push(serde_json::json!({
            "kind": "preserve_historical_transition",
            "path": full_path,
            "job_id": transition.job_id,
        }));
    }
    Ok(())
}
