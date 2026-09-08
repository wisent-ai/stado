//! Priority-index markers and status companions: the two families that carry
//! no job document of their own and must be resolved against the typed job
//! their body or their key names.

use serde_json::Value;
use std::collections::HashSet;

use crate::monitor::reap::reads::{required_snapshot_text, terminal_snapshot_present};
use crate::queue::{JobStorage, StorageError};

use super::{LifecyclePath, SnapshotIndex};

pub(super) async fn classify_priority_marker(
    store: &JobStorage,
    index: &SnapshotIndex,
    emitted_cancellations: &mut HashSet<String>,
    decisions: &mut Vec<Value>,
    path: LifecyclePath<'_>,
) -> Result<(), StorageError> {
    let SnapshotIndex {
        retained_jobs,
        queued_cancellations,
        ..
    } = index;
    let LifecyclePath {
        relative,
        full_path,
        ..
    } = path;
    if !crate::queue::listing::is_marker(relative) {
        decisions.push(serde_json::json!({
            "kind": "preserve_historical",
            "path": full_path,
            "reason": "canonical priority-index bookkeeping marker",
        }));
        return Ok(());
    }
    let raw = required_snapshot_text(store, relative).await?;
    let marker: Value = serde_json::from_str(&raw)?;
    let job_id = marker
        .get("job_id")
        .and_then(Value::as_str)
        .filter(|job_id| !job_id.is_empty())
        .ok_or_else(|| StorageError::Other(format!("{relative} has no job_id")))?;
    let priority = marker
        .get("priority")
        .and_then(Value::as_i64)
        .ok_or_else(|| StorageError::Other(format!("{relative} has no integer priority")))?;
    let queued = store.read_job("queue", job_id).await?;
    if let Some(job) = queued.as_ref() {
        if job.priority != priority || crate::queue::listing::marker_path(job) != *relative {
            return Err(StorageError::Other(format!(
                "{relative} disagrees with its typed queued job"
            )));
        }
    }
    if queued_cancellations.contains(job_id) {
        if emitted_cancellations.insert(job_id.to_string()) {
            decisions.push(serde_json::json!({
                "kind": "queued_cancellation",
                "job_id": job_id,
            }));
        }
    } else if let Some(run_id) = retained_jobs.get(job_id) {
        decisions.push(serde_json::json!({
            "kind": "retained_outcome_cleanup",
            "run_id": run_id,
            "job_id": job_id,
            "primary_only_paths": [full_path],
            "transition_companions": [],
        }));
    } else if queued.is_none() && terminal_snapshot_present(store, job_id).await? {
        decisions.push(serde_json::json!({
            "kind": "preserve_historical",
            "path": full_path,
            "job_id": job_id,
        }));
    } else {
        decisions.push(serde_json::json!({
            "kind": "block_unclassified_live",
            "path": full_path,
            "job_id": job_id,
            "reason": "priority marker still belongs to live queued work",
        }));
    }
    Ok(())
}

pub(super) async fn classify_status_companion(
    store: &JobStorage,
    index: &SnapshotIndex,
    decisions: &mut Vec<Value>,
    path: LifecyclePath<'_>,
) -> Result<(), StorageError> {
    let SnapshotIndex { retained_jobs, .. } = index;
    let LifecyclePath {
        relative,
        tail,
        full_path,
        ..
    } = path;
    let parts = tail.split('/').collect::<Vec<_>>();
    if parts.len() != 2 || !matches!(parts[1], "status" | "heartbeat") {
        return Err(StorageError::Other(format!(
            "invalid status lifecycle key {relative}"
        )));
    }
    let job_id = parts[0];
    if let Some(run_id) = retained_jobs.get(job_id) {
        decisions.push(serde_json::json!({
            "kind": "retained_outcome_cleanup",
            "run_id": run_id,
            "job_id": job_id,
            "primary_only_paths": [full_path],
            "transition_companions": [],
        }));
    } else if terminal_snapshot_present(store, job_id).await? {
        decisions.push(serde_json::json!({
            "kind": "preserve_historical",
            "path": full_path,
            "job_id": job_id,
        }));
    } else {
        decisions.push(serde_json::json!({
            "kind": "block_unclassified_live",
            "path": full_path,
            "job_id": job_id,
            "reason": "status object belongs to live or unclassified work",
        }));
    }
    Ok(())
}
