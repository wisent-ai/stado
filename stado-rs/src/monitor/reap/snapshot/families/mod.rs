//! The lifecycle indexes a family classifier reads, the canonical key it is
//! handed, and the dispatch from one such key to the family that owns it.

use serde_json::Value;
use std::collections::HashSet;

use crate::monitor::reap::manifest::has_complete_retained_outcomes;
use crate::monitor::reap::reads::required_snapshot_text;
use crate::queue::{JobStorage, StorageError};

mod jobs;
mod markers;
mod runs;

/// Every durable index a family classifier consults: the jobs whose outcomes
/// a validated v3 run manifest already retains, the run manifests themselves,
/// and the cancellation markers whose job is still queued.
pub(super) struct SnapshotIndex {
    retained_jobs: std::collections::BTreeMap<String, String>,
    run_documents: std::collections::BTreeMap<String, Value>,
    queued_cancellations: HashSet<String>,
}

/// One destination-only lifecycle key, split into the parts a classifier
/// names: the store-relative key, its canonical family, the tail below that
/// family, and the reported full path.
#[derive(Clone, Copy)]
pub(super) struct LifecyclePath<'a> {
    pub(super) relative: &'a str,
    pub(super) family: &'a str,
    pub(super) tail: &'a str,
    pub(super) full_path: &'a str,
}

pub(super) async fn read_snapshot_index(store: &JobStorage) -> Result<SnapshotIndex, StorageError> {
    let mut retained_jobs = std::collections::BTreeMap::<String, String>::new();
    let mut run_documents = std::collections::BTreeMap::<String, Value>::new();
    for run_id in crate::queue::runs::list_runs(store).await? {
        let path = format!("runs/{run_id}.json");
        let raw = required_snapshot_text(store, &path).await?;
        let document: Value = serde_json::from_str(&raw)?;
        if document.get("schema").and_then(Value::as_str) == Some("stado.run-submission.v3") {
            crate::queue::submit::validate_stored_run_manifest(&document, &run_id)
                .map_err(|error| StorageError::Other(error.to_string()))?;
            if has_complete_retained_outcomes(&document) {
                for entry in document
                    .get("entries")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                {
                    if let Some(job_id) = entry.get("job_id").and_then(Value::as_str) {
                        retained_jobs.insert(job_id.to_string(), run_id.clone());
                    }
                }
            }
        }
        run_documents.insert(run_id, document);
    }

    let mut queued_cancellations = HashSet::new();
    for path in store.list_paths("cancellations/", 0).await? {
        let Some(job_id) = path
            .strip_prefix("cancellations/")
            .and_then(|tail| tail.strip_suffix(".json"))
        else {
            continue;
        };
        let marker = required_snapshot_text(store, &path).await?;
        crate::queue::storage::validate_cancellation_snapshot(job_id, &marker)?;
        if store
            .read_job("queue", job_id)
            .await?
            .is_some_and(|job| job.state == crate::models::job_state::QUEUED)
        {
            queued_cancellations.insert(job_id.to_string());
        }
    }

    Ok(SnapshotIndex {
        retained_jobs,
        run_documents,
        queued_cancellations,
    })
}

/// Route one lifecycle key to the family that owns its typed contract. A key
/// no family claims is never deleted: it blocks as unclassified live work.
pub(super) async fn classify_lifecycle_path(
    store: &JobStorage,
    index: &SnapshotIndex,
    emitted_cancellations: &mut HashSet<String>,
    decisions: &mut Vec<Value>,
    path: LifecyclePath<'_>,
) -> Result<(), StorageError> {
    let LifecyclePath {
        family, full_path, ..
    } = path;
    if family == "runs" {
        runs::classify_run_manifest(store, index, decisions, path).await?;
        return Ok(());
    }

    if family == "job-transitions" {
        runs::classify_transition_companion(store, index, decisions, path).await?;
        return Ok(());
    }

    if crate::queue::runs::ALL_PREFIXES.contains(&family) {
        jobs::classify_typed_job(store, index, emitted_cancellations, decisions, path).await?;
        return Ok(());
    }

    if family == "cancellations" {
        jobs::classify_cancellation_marker(store, index, emitted_cancellations, decisions, path)
            .await?;
        return Ok(());
    }

    if family == "queue_priority" {
        markers::classify_priority_marker(store, index, emitted_cancellations, decisions, path)
            .await?;
        return Ok(());
    }

    if family == "status" {
        markers::classify_status_companion(store, index, decisions, path).await?;
        return Ok(());
    }

    decisions.push(serde_json::json!({
        "kind": "block_unclassified_live",
        "path": full_path,
        "reason": "lifecycle companion has no production typed reconciliation contract",
    }));
    Ok(())
}
