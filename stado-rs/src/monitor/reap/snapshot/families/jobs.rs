//! Typed job snapshots and cancellation markers: the two families whose
//! classification turns on whether a job is still queued, already retained,
//! or provably retired.

use serde_json::Value;
use std::collections::HashSet;

use crate::monitor::reap::reads::{required_snapshot_text, terminal_snapshot_present};
use crate::queue::{JobStorage, StorageError};

use super::{LifecyclePath, SnapshotIndex};

pub(super) async fn classify_typed_job(
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
        family,
        tail,
        full_path,
    } = path;
    let Some(job_id) = tail.strip_suffix(".json") else {
        decisions.push(serde_json::json!({
            "kind": "block_unclassified_live",
            "path": full_path,
            "reason": "job key is not canonical",
        }));
        return Ok(());
    };
    let raw = required_snapshot_text(store, relative).await?;
    let job = crate::models::Job::from_json(&raw)?;
    if job.job_id != job_id {
        return Err(StorageError::Other(format!(
            "{relative} contains a different job identity"
        )));
    }
    if queued_cancellations.contains(job_id)
        && matches!(family, "queue" | "cancellations" | "queue_priority")
    {
        if emitted_cancellations.insert(job_id.to_string()) {
            decisions.push(serde_json::json!({
                "kind": "queued_cancellation",
                "job_id": job_id,
            }));
        }
        return Ok(());
    }
    if let Some(run_id) = retained_jobs.get(job_id) {
        decisions.push(serde_json::json!({
            "kind": "retained_outcome_cleanup",
            "run_id": run_id,
            "job_id": job_id,
            "primary_only_paths": [full_path],
            "transition_companions": [],
        }));
        return Ok(());
    }
    let expected_state = if family == "queue" {
        crate::models::job_state::QUEUED
    } else {
        family
    };
    if crate::queue::runs::TERMINAL_PREFIXES.contains(&family) && job.state == expected_state {
        decisions.push(serde_json::json!({
            "kind": "preserve_historical",
            "path": full_path,
            "job_id": job_id,
        }));
    } else if crate::queue::storage::is_transition_sentinel_state(&job.state)
        && store.workdir_job_state(job_id).await?
            == crate::queue::storage::WorkdirJobState::Terminal
    {
        decisions.push(serde_json::json!({
            "kind": "preserve_historical_transition",
            "path": full_path,
            "job_id": job_id,
        }));
    } else {
        decisions.push(serde_json::json!({
            "kind": "block_unclassified_live",
            "path": full_path,
            "job_id": job_id,
            "reason": "typed job state is live or lacks canonical terminal transition proof",
        }));
    }
    Ok(())
}

pub(super) async fn classify_cancellation_marker(
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
        tail,
        full_path,
        ..
    } = path;
    let Some(job_id) = tail.strip_suffix(".json") else {
        return Err(StorageError::Other(format!(
            "invalid cancellation key {relative}"
        )));
    };
    let raw = required_snapshot_text(store, relative).await?;
    crate::queue::storage::validate_cancellation_snapshot(job_id, &raw)?;
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
            "reason": "cancellation marker has neither queued nor terminal job proof",
        }));
    }
    Ok(())
}
