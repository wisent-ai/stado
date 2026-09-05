//! By-run reaper: removes per-job cruft once a run is fully terminal.
//!
//! Port of `stado/monitor/reap/run_reaper.py`.
//!
//! A run is reapable when every durable entry has reached a terminal prefix.
//! Reaping first CAS-retains each exact terminal job in the manifest and marks
//! entries reaped; only that committed snapshot permits lifecycle blob
//! deletion.
//!
//! Retention and cleanup are two separate durable facts, because a crash sits
//! between them. [`REAPED_AT`] says the outcomes are retained; only
//! [`CLEANUP_COMPLETED_AT`] says every lifecycle blob and status entry is
//! gone. A manifest carrying the first without the second is a legal state —
//! the run-manifest schema admits both keys — and it re-enters a
//! deletion-only pass that retains nothing again, rewrites no `reaped_at`,
//! and tolerates blobs that are already deleted.

use chrono::Utc;
use serde_json::{Map, Value};
use std::collections::HashSet;

use crate::queue::runs::{
    list_runs, record_terminal_outcome_for_entry, run_status, ALL_PREFIXES, RUN_PREFIX,
    TERMINAL_PREFIXES,
};
use crate::queue::{JobStorage, StorageError};

/// A run retained because its durable schema cannot support destructive reap.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReapRefusal {
    pub run_id: String,
    pub reason: &'static str,
}

/// Python summary dict {"reaped_runs", "deleted_jobs", "examined_runs"}.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReapSummary {
    pub reaped_runs: i64,
    pub deleted_jobs: i64,
    pub examined_runs: i64,
    pub refused_runs: Vec<ReapRefusal>,
}

fn snapshot_full_path(relative: &str) -> String {
    format!("ecosystem/probierz/{relative}")
}

async fn required_snapshot_text(store: &JobStorage, path: &str) -> Result<String, StorageError> {
    store
        .download_text(path)
        .await?
        .ok_or_else(|| StorageError::NotFound(path.to_string()))
}

async fn terminal_snapshot_present(store: &JobStorage, job_id: &str) -> Result<bool, StorageError> {
    for prefix in crate::queue::runs::TERMINAL_PREFIXES {
        if store
            .read_job(prefix, job_id)
            .await?
            .is_some_and(|job| job.job_id == job_id && job.state == prefix)
        {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Classify destination-only lifecycle objects from a sealed B-winning A/B
/// snapshot using the queue's production job, transition, cancellation, run,
/// and retained-outcome contracts. This function is read-only by construction:
/// callers bind `store` to an immutable local checkpoint.
pub(crate) async fn classify_reconciliation_snapshot(
    store: &JobStorage,
    primary_only_paths: &[String],
) -> Result<Vec<Value>, StorageError> {
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

    let mut decisions = Vec::new();
    let mut emitted_cancellations = HashSet::new();
    for relative in primary_only_paths {
        let Some((family, tail)) = relative.split_once('/') else {
            decisions.push(serde_json::json!({
                "kind": "block_unclassified_live",
                "path": snapshot_full_path(relative),
                "reason": "lifecycle object has no canonical family key",
            }));
            continue;
        };
        let full_path = snapshot_full_path(relative);
        if family == "runs" {
            let Some(run_id) = tail.strip_suffix(".json") else {
                decisions.push(serde_json::json!({
                    "kind": "block_unclassified_live",
                    "path": full_path,
                    "reason": "run manifest key is not canonical",
                }));
                continue;
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
            continue;
        }

        if family == "job-transitions" {
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
            continue;
        }

        if crate::queue::runs::ALL_PREFIXES.contains(&family) {
            let Some(job_id) = tail.strip_suffix(".json") else {
                decisions.push(serde_json::json!({
                    "kind": "block_unclassified_live",
                    "path": full_path,
                    "reason": "job key is not canonical",
                }));
                continue;
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
                continue;
            }
            if let Some(run_id) = retained_jobs.get(job_id) {
                decisions.push(serde_json::json!({
                    "kind": "retained_outcome_cleanup",
                    "run_id": run_id,
                    "job_id": job_id,
                    "primary_only_paths": [full_path],
                    "transition_companions": [],
                }));
                continue;
            }
            let expected_state = if family == "queue" {
                crate::models::job_state::QUEUED
            } else {
                family
            };
            if crate::queue::runs::TERMINAL_PREFIXES.contains(&family)
                && job.state == expected_state
            {
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
            continue;
        }

        if family == "cancellations" {
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
            continue;
        }

        if family == "queue_priority" {
            if !crate::queue::listing::is_marker(relative) {
                decisions.push(serde_json::json!({
                    "kind": "preserve_historical",
                    "path": full_path,
                    "reason": "canonical priority-index bookkeeping marker",
                }));
                continue;
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
                .ok_or_else(|| {
                    StorageError::Other(format!("{relative} has no integer priority"))
                })?;
            let queued = store.read_job("queue", job_id).await?;
            if let Some(job) = queued.as_ref() {
                if job.priority != priority || crate::queue::listing::marker_path(job) != *relative
                {
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
            continue;
        }

        if family == "status" {
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
            continue;
        }

        decisions.push(serde_json::json!({
            "kind": "block_unclassified_live",
            "path": full_path,
            "reason": "lifecycle companion has no production typed reconciliation contract",
        }));
    }
    Ok(decisions)
}
fn reconciliation_store_path(path: &str) -> Result<&str, StorageError> {
    path.strip_prefix("ecosystem/probierz/")
        .ok_or_else(|| StorageError::Other(format!("non-canonical lifecycle path {path}")))
}

async fn prove_snapshot_content_retained(
    live: &JobStorage,
    snapshot: &JobStorage,
    path: &str,
) -> Result<String, StorageError> {
    let path = reconciliation_store_path(path)?;
    let expected = required_snapshot_text(snapshot, path).await?;
    let actual = required_snapshot_text(live, path).await?;
    if actual != expected {
        return Err(StorageError::Other(format!(
            "retained lifecycle content changed at {path}"
        )));
    }
    Ok(actual)
}

/// Re-observe completed reconciliation decisions through the same production
/// types that classified the immutable snapshot. This is intentionally not a
/// path/sentinel checker: retained content is compared with the sealed
/// checkpoint, while jobs, cancellations, runs, and transitions are parsed by
/// their owning Rust contracts.
pub(crate) async fn validate_reconciliation_final_state(
    live: &JobStorage,
    snapshot: &JobStorage,
    decisions: &[Value],
) -> Result<Vec<Value>, StorageError> {
    let mut observations = Vec::with_capacity(decisions.len());
    for decision in decisions {
        let kind = decision
            .get("kind")
            .and_then(Value::as_str)
            .ok_or_else(|| StorageError::Other("lifecycle decision has no kind".to_string()))?;
        match kind {
            "queued_cancellation" => {
                let job_id = decision
                    .get("job_id")
                    .and_then(Value::as_str)
                    .ok_or_else(|| {
                        StorageError::Other(
                            "queued cancellation decision has no job_id".to_string(),
                        )
                    })?;
                if live.read_job("queue", job_id).await?.is_some() {
                    return Err(StorageError::Other(format!(
                        "queued cancellation for {job_id} remains queued"
                    )));
                }
                let cancelled = live
                    .read_job("cancelled", job_id)
                    .await?
                    .filter(|job| job.job_id == job_id && job.state == "cancelled")
                    .ok_or_else(|| {
                        StorageError::Other(format!(
                            "queued cancellation for {job_id} has no typed cancelled result"
                        ))
                    })?;
                let cancellation_path = format!("cancellations/{job_id}.json");
                if let Some(marker) = live.download_text(&cancellation_path).await? {
                    crate::queue::storage::validate_cancellation_snapshot(job_id, &marker)?;
                }
                observations.push(serde_json::json!({
                    "kind": kind,
                    "job_id": job_id,
                    "state": cancelled.state,
                    "typed": true,
                }));
            }
            "retained_outcome_cleanup" => {
                for path in decision
                    .get("primary_only_paths")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                {
                    let store_path = reconciliation_store_path(path)?;
                    if live.backend().exists(store_path).await? {
                        return Err(StorageError::Other(format!(
                            "retained-outcome cleanup left {store_path}"
                        )));
                    }
                }
                for path in decision
                    .get("transition_companions")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                {
                    let store_path = reconciliation_store_path(path)?;
                    let raw = required_snapshot_text(live, store_path).await?;
                    let transition =
                        crate::queue::storage::validate_transition_snapshot(store_path, &raw)?;
                    if !transition.retired {
                        return Err(StorageError::Other(format!(
                            "transition companion {store_path} is not retired"
                        )));
                    }
                }
                observations.push(serde_json::json!({"kind": kind, "typed": true}));
            }
            "terminal_run_recovery" => {
                let path = decision
                    .get("path")
                    .and_then(Value::as_str)
                    .ok_or_else(|| {
                        StorageError::Other("terminal run decision has no path".to_string())
                    })?;
                let store_path = reconciliation_store_path(path)?;
                let run_id = store_path
                    .strip_prefix("runs/")
                    .and_then(|tail| tail.strip_suffix(".json"))
                    .ok_or_else(|| {
                        StorageError::Other(format!("non-canonical run path {store_path}"))
                    })?;
                let raw = required_snapshot_text(live, store_path).await?;
                let manifest: Value = serde_json::from_str(&raw)?;
                crate::queue::submit::validate_stored_run_manifest(&manifest, run_id)
                    .map_err(|error| StorageError::Other(error.to_string()))?;
                if !has_complete_retained_outcomes(&manifest)
                    || !manifest.get(CLEANUP_COMPLETED_AT).is_some_and(py_truthy)
                {
                    return Err(StorageError::Other(format!(
                        "terminal run {run_id} lacks complete retained outcomes"
                    )));
                }
                observations.push(serde_json::json!({
                    "kind": kind,
                    "path": path,
                    "schema": "stado.run-submission.v3",
                    "typed": true,
                }));
            }
            "preserve_historical_run" => {
                let path = decision
                    .get("path")
                    .and_then(Value::as_str)
                    .ok_or_else(|| {
                        StorageError::Other("historical run decision has no path".to_string())
                    })?;
                let store_path = reconciliation_store_path(path)?;
                let run_id = store_path
                    .strip_prefix("runs/")
                    .and_then(|tail| tail.strip_suffix(".json"))
                    .ok_or_else(|| {
                        StorageError::Other(format!("non-canonical run path {store_path}"))
                    })?;
                let expected_raw = required_snapshot_text(snapshot, store_path).await?;
                let expected: Value = serde_json::from_str(&expected_raw)?;
                let schema = expected
                    .get("schema")
                    .and_then(Value::as_str)
                    .unwrap_or("legacy-unversioned");
                if schema == "stado.run-submission.v3" {
                    let live_raw = required_snapshot_text(live, store_path).await?;
                    let manifest: Value = serde_json::from_str(&live_raw)?;
                    crate::queue::submit::validate_stored_run_manifest(&manifest, run_id)
                        .map_err(|error| StorageError::Other(error.to_string()))?;
                    if !has_complete_retained_outcomes(&manifest) {
                        return Err(StorageError::Other(format!(
                            "retained run {run_id} lost complete typed outcomes"
                        )));
                    }
                } else {
                    prove_snapshot_content_retained(live, snapshot, path).await?;
                }
                observations.push(serde_json::json!({
                    "kind": kind,
                    "path": path,
                    "schema": schema,
                    "content_retained": schema != "stado.run-submission.v3",
                    "typed": schema == "stado.run-submission.v3",
                }));
            }
            "preserve_historical_transition" => {
                let path = decision
                    .get("path")
                    .and_then(Value::as_str)
                    .ok_or_else(|| {
                        StorageError::Other(
                            "historical transition decision has no path".to_string(),
                        )
                    })?;
                let raw = prove_snapshot_content_retained(live, snapshot, path).await?;
                let store_path = reconciliation_store_path(path)?;
                if store_path.starts_with("job-transitions/") {
                    let transition =
                        crate::queue::storage::validate_transition_snapshot(store_path, &raw)?;
                    if !transition.retired {
                        return Err(StorageError::Other(format!(
                            "historical transition {store_path} is not retired"
                        )));
                    }
                } else {
                    let job = crate::models::Job::from_json(&raw)?;
                    if live.workdir_job_state(&job.job_id).await?
                        != crate::queue::storage::WorkdirJobState::Terminal
                    {
                        return Err(StorageError::Other(format!(
                            "historical transition job {} lacks retired typed proof",
                            job.job_id
                        )));
                    }
                }
                observations.push(serde_json::json!({
                    "kind": kind,
                    "path": path,
                    "content_retained": true,
                    "typed": true,
                }));
            }
            "preserve_historical" => {
                let path = decision
                    .get("path")
                    .and_then(Value::as_str)
                    .ok_or_else(|| {
                        StorageError::Other("historical lifecycle decision has no path".to_string())
                    })?;
                let raw = prove_snapshot_content_retained(live, snapshot, path).await?;
                let store_path = reconciliation_store_path(path)?;
                if let Some(job_id) = store_path
                    .strip_prefix("cancellations/")
                    .and_then(|tail| tail.strip_suffix(".json"))
                {
                    crate::queue::storage::validate_cancellation_snapshot(job_id, &raw)?;
                } else if TERMINAL_PREFIXES
                    .iter()
                    .any(|prefix| store_path.starts_with(&format!("{prefix}/")))
                {
                    let job = crate::models::Job::from_json(&raw)?;
                    if !TERMINAL_PREFIXES.contains(&job.state.as_str()) {
                        return Err(StorageError::Other(format!(
                            "historical terminal job {} is not terminal",
                            job.job_id
                        )));
                    }
                }
                observations.push(serde_json::json!({
                    "kind": kind,
                    "path": path,
                    "content_retained": true,
                    "typed": true,
                }));
            }
            "block_unclassified_live" => {
                return Err(StorageError::Other(format!(
                    "unclassified live lifecycle decision reached finalization: {}",
                    decision
                        .get("path")
                        .and_then(Value::as_str)
                        .unwrap_or("<unknown>")
                )));
            }
            other => {
                return Err(StorageError::Other(format!(
                    "unknown lifecycle decision {other}"
                )));
            }
        }
    }
    Ok(observations)
}

/// Python truthiness for the `manifest.get("reaped_at")` skip check.
fn py_truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().is_some_and(|f| f != 0.0),
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}

/// Delete every blob under status/<job_id>/. One failing delete does not
/// abandon the rest: the whole directory has to go before cleanup can be
/// recorded, so the pass deletes what it can and reports the first failure for
/// the next pass to resume.
async fn delete_status_dir(store: &JobStorage, job_id: &str) -> Result<(), StorageError> {
    let mut failure = None;
    for path in store.list_paths(&format!("status/{job_id}/"), 0).await? {
        if let Err(error) = store.delete_blob(&path).await {
            failure = failure.or(Some(error));
        }
    }
    match failure {
        Some(error) => Err(error),
        None => Ok(()),
    }
}

/// Manifest key: the outcomes of every entry are durably retained.
const REAPED_AT: &str = "reaped_at";
/// Manifest key: every lifecycle blob and status entry of a retained run has
/// been deleted. Written only after that deletion succeeded in full.
const CLEANUP_COMPLETED_AT: &str = "cleanup_completed_at";

/// Job ids of a manifest's durable entries.
fn manifest_job_ids(manifest: &Value, run_id: &str) -> Result<Vec<String>, StorageError> {
    manifest
        .get("entries")
        .and_then(Value::as_array)
        .ok_or_else(|| {
            StorageError::Other(format!("run manifest {run_id} missing durable entries"))
        })?
        .iter()
        .map(|entry| {
            entry
                .get("job_id")
                .and_then(Value::as_str)
                .map(str::to_string)
                .ok_or_else(|| {
                    StorageError::Other(format!("run manifest {run_id} has an invalid entry"))
                })
        })
        .collect()
}
/// A cleanup marker is portable and therefore cannot stand in for the
/// retained outcomes themselves. Every entry must carry the exact terminal
/// projection the normal retention path records before queue/running residue
/// may be deleted on this destination.
fn has_complete_retained_outcomes(manifest: &Value) -> bool {
    let Some(entries) = manifest.get("entries").and_then(Value::as_array) else {
        return false;
    };
    !entries.is_empty()
        && entries.iter().all(|entry| {
            let Some(job_id) = entry.get("job_id").and_then(Value::as_str) else {
                return false;
            };
            let Some(outcome) = entry.get("outcome").and_then(Value::as_object) else {
                return false;
            };
            let Some(prefix) = outcome.get("prefix").and_then(Value::as_str) else {
                return false;
            };
            TERMINAL_PREFIXES.contains(&prefix)
                && outcome
                    .get("job")
                    .and_then(Value::as_object)
                    .is_some_and(|job| {
                        job.get("job_id").and_then(Value::as_str) == Some(job_id)
                            && job.get("state").and_then(Value::as_str) == Some(prefix)
                    })
        })
}

/// Build one lifecycle-residue index per tick. Exact lifecycle/status paths
/// encode the job id; priority and transition companions state it in their
/// JSON body, so they are read once here rather than searched once per run.
async fn cleanup_residue_job_ids(store: &JobStorage) -> Result<HashSet<String>, StorageError> {
    let mut job_ids = HashSet::new();
    for prefix in ALL_PREFIXES {
        let start = format!("{prefix}/");
        for path in store.list_paths(&start, 0).await? {
            if let Some(job_id) = path
                .strip_prefix(&start)
                .and_then(|tail| tail.strip_suffix(".json"))
                .filter(|tail| !tail.is_empty() && !tail.contains('/'))
            {
                job_ids.insert(job_id.to_string());
            }
        }
    }
    for path in store.list_paths("status/", 0).await? {
        if let Some(job_id) = path
            .strip_prefix("status/")
            .and_then(|tail| tail.split('/').next())
            .filter(|job_id| !job_id.is_empty())
        {
            job_ids.insert(job_id.to_string());
        }
    }
    for prefix in ["queue_priority/", "job-transitions/"] {
        for path in store.list_paths(prefix, 0).await? {
            if prefix == "queue_priority/" && !crate::queue::listing::is_marker(&path) {
                continue;
            }
            let Some(body) = store.download_text(&path).await? else {
                continue;
            };
            let document: Value = serde_json::from_str(&body).map_err(|error| {
                StorageError::Other(format!("invalid lifecycle companion {path}: {error}"))
            })?;
            let job_id = document
                .get("job_id")
                .and_then(Value::as_str)
                .filter(|job_id| !job_id.is_empty())
                .ok_or_else(|| {
                    StorageError::Other(format!("lifecycle companion {path} has no job_id"))
                })?;
            if prefix == "job-transitions/"
                && document
                    .get("state")
                    .and_then(Value::as_str)
                    .is_some_and(crate::queue::storage::transition_is_retired)
            {
                continue;
            }
            job_ids.insert(job_id.to_string());
        }
    }
    Ok(job_ids)
}

fn retained_run_has_residue(residue_job_ids: &HashSet<String>, job_ids: &[String]) -> bool {
    job_ids
        .iter()
        .any(|job_id| residue_job_ids.contains(job_id))
}

/// Delete every lifecycle blob and status entry of a retained run, then record
/// that the cleanup finished. Idempotent: a blob another pass already removed
/// is simply absent, and the completion marker is written only once every
/// deletion of this pass succeeded, so an interrupted sweep is resumed rather
/// than abandoned. Returns how many blobs this pass deleted.
async fn sweep_retained_run(
    store: &JobStorage,
    run_id: &str,
    job_ids: &[String],
) -> Result<i64, StorageError> {
    let path = format!("{RUN_PREFIX}/{run_id}.json");
    let Some(retained) = store.read_text_versioned(&path).await? else {
        return Ok(0);
    };
    let retained_manifest: Value = serde_json::from_str(&retained.content)?;
    crate::queue::submit::validate_stored_run_manifest(&retained_manifest, run_id)
        .map_err(|error| StorageError::Other(error.to_string()))?;
    if !has_complete_retained_outcomes(&retained_manifest) {
        return Err(StorageError::Other(format!(
            "run {run_id} cannot be swept without complete retained terminal outcomes"
        )));
    }
    let mut deleted = 0;
    for job_id in job_ids {
        // A prepared transition is a lifecycle companion, not an independent
        // source of truth. Resolve it through the canonical transition
        // protocol against the retained terminal destination before removing
        // any stale projection; never patch a job document here.
        store.recover_job_transition(job_id).await?;
        for prefix in TERMINAL_PREFIXES
            .iter()
            .copied()
            .chain(["queue", "running"])
        {
            let blob = format!("{prefix}/{job_id}.json");
            if store.backend().exists(&blob).await? {
                store.delete_job(prefix, job_id).await?;
                deleted += 1;
            }
        }
        store.repair_priority_markers(job_id, None).await?;
        delete_status_dir(store, job_id).await?;
    }
    for _ in 0..16 {
        let Some(versioned) = store.read_text_versioned(&path).await? else {
            return Ok(deleted);
        };
        let mut manifest: Value = serde_json::from_str(&versioned.content)?;
        let object = manifest.as_object_mut().ok_or_else(|| {
            StorageError::Other(format!("run manifest {run_id} is not a JSON object"))
        })?;
        if object.get(CLEANUP_COMPLETED_AT).is_some_and(py_truthy) {
            return Ok(deleted);
        }
        object.insert(
            CLEANUP_COMPLETED_AT.into(),
            Value::from(Utc::now().format("%Y-%m-%dT%H:%M:%S+00:00").to_string()),
        );
        match store
            .compare_and_swap_text(
                &path,
                &versioned.version,
                &serde_json::to_string_pretty(&manifest)?,
            )
            .await
        {
            Ok(_) => return Ok(deleted),
            Err(StorageError::StorageConflict(_)) => continue,
            Err(StorageError::NotFound(_)) => return Ok(deleted),
            Err(error) => return Err(error),
        }
    }
    Err(StorageError::StorageConflict(format!(
        "run manifest {run_id} remained contended while recording cleanup completion"
    )))
}

/// Reap all fully-terminal runs. Returns a summary.
///
/// `limit > 0` caps how many runs this tick touches — a fresh reap or a
/// resumed cleanup both count against it, so an interrupted backlog cannot
/// make one tick unbounded; 0 means no cap.
pub async fn reap_terminal_runs(
    store: &JobStorage,
    limit: i64,
) -> Result<ReapSummary, StorageError> {
    let mut summary = ReapSummary::default();
    let mut touched = 0;
    let cleanup_residue = cleanup_residue_job_ids(store).await?;
    for run_id in list_runs(store).await? {
        let path = format!("{RUN_PREFIX}/{run_id}.json");
        let Some(initial) = store.read_text_versioned(&path).await? else {
            continue;
        };
        let initial_manifest: Value = serde_json::from_str(&initial.content)?;
        if initial_manifest.get("schema").and_then(Value::as_str) != Some("stado.run-submission.v3")
        {
            summary.refused_runs.push(ReapRefusal {
                run_id,
                reason: "unsupported legacy manifest schema; retained without cleanup",
            });
            continue;
        }
        crate::queue::submit::validate_stored_run_manifest(&initial_manifest, &run_id)
            .map_err(|error| StorageError::Other(error.to_string()))?;
        if initial_manifest
            .get(CLEANUP_COMPLETED_AT)
            .is_some_and(py_truthy)
        {
            if !has_complete_retained_outcomes(&initial_manifest) {
                summary.refused_runs.push(ReapRefusal {
                    run_id,
                    reason: "cleanup marker lacks complete retained terminal outcomes; retained without cleanup",
                });
                continue;
            }
            let job_ids = manifest_job_ids(&initial_manifest, &run_id)?;
            if !retained_run_has_residue(&cleanup_residue, &job_ids) {
                continue;
            }
            summary.deleted_jobs += sweep_retained_run(store, &run_id, &job_ids).await?;
            touched += 1;
            if limit > 0 && touched >= limit {
                break;
            }
            continue;
        }
        if initial_manifest.get(REAPED_AT).is_some_and(py_truthy) {
            if !has_complete_retained_outcomes(&initial_manifest) {
                summary.refused_runs.push(ReapRefusal {
                    run_id,
                    reason: "reaped marker lacks complete retained terminal outcomes; retained without cleanup",
                });
                continue;
            }
            // Retained, but the deletion pass that follows retention did not
            // finish. Resume exactly that: no outcome is retained again and
            // `reaped_at` is not rewritten, so this run is not counted as a
            // second reap.
            let job_ids = manifest_job_ids(&initial_manifest, &run_id)?;
            summary.deleted_jobs += sweep_retained_run(store, &run_id, &job_ids).await?;
            touched += 1;
            if limit > 0 && touched >= limit {
                break;
            }
            continue;
        }
        summary.examined_runs += 1;
        let Some(status) = run_status(store, &run_id).await? else {
            continue;
        };
        if !status.all_terminal {
            continue;
        }

        let entries = initial_manifest
            .get("entries")
            .and_then(Value::as_array)
            .ok_or_else(|| {
                StorageError::Other(format!("run manifest {run_id} missing durable entries"))
            })?;
        for (index, entry) in entries.iter().enumerate() {
            if entry.get("outcome").is_some_and(Value::is_object) {
                continue;
            }
            let job_id = entry.get("job_id").and_then(Value::as_str).ok_or_else(|| {
                StorageError::Other(format!("run manifest {run_id} has an invalid entry"))
            })?;
            let mut found = None;
            for prefix in TERMINAL_PREFIXES {
                if let Some(job) = store.read_job(prefix, job_id).await? {
                    found = Some((prefix, job));
                    break;
                }
            }
            let Some((prefix, job)) = found else {
                return Err(StorageError::Other(format!(
                    "terminal job {job_id} disappeared before run {run_id} retained its outcome"
                )));
            };
            record_terminal_outcome_for_entry(store, &run_id, index, &job, prefix).await?;
        }

        let Some(versioned) = store.read_text_versioned(&path).await? else {
            continue;
        };
        let mut manifest: Value = serde_json::from_str(&versioned.content)?;
        crate::queue::submit::validate_stored_run_manifest(&manifest, &run_id)
            .map_err(|error| StorageError::Other(error.to_string()))?;
        let entries = manifest
            .get_mut("entries")
            .and_then(Value::as_array_mut)
            .ok_or_else(|| {
                StorageError::Other(format!("run manifest {run_id} missing durable entries"))
            })?;
        let mut job_ids = Vec::with_capacity(entries.len());
        let reaped_at = Utc::now().format("%Y-%m-%dT%H:%M:%S+00:00").to_string();
        for entry in entries {
            if !entry.get("outcome").is_some_and(Value::is_object) {
                return Err(StorageError::Other(format!(
                    "run manifest {run_id} has a terminal entry without retained outcome"
                )));
            }
            let job_id = entry
                .get("job_id")
                .and_then(Value::as_str)
                .ok_or_else(|| {
                    StorageError::Other(format!("run manifest {run_id} has an invalid entry"))
                })?
                .to_string();
            job_ids.push(job_id);
            let entry = entry
                .as_object_mut()
                .expect("validated durable entry object");
            entry.insert("state".into(), Value::from("reaped"));
            entry.insert(REAPED_AT.into(), Value::from(reaped_at.as_str()));
        }
        let manifest_object = manifest
            .as_object_mut()
            .expect("validated run manifest object");
        manifest_object.insert(REAPED_AT.into(), Value::from(reaped_at));
        let counts: Map<String, Value> = ALL_PREFIXES
            .iter()
            .map(|prefix| (prefix.to_string(), Value::from(status.counts[*prefix])))
            .collect();
        manifest_object.insert("final_counts".into(), Value::Object(counts));
        match store
            .compare_and_swap_text(
                &path,
                &versioned.version,
                &serde_json::to_string_pretty(&manifest)?,
            )
            .await
        {
            Ok(_) => {}
            Err(StorageError::StorageConflict(_) | StorageError::NotFound(_)) => continue,
            Err(error) => return Err(error),
        }
        // The manifest is the deletion fence. Another coordinator may retire
        // it after our successful CAS; in that case do not guess a terminal
        // state and, critically, do not delete any job blobs. A storage read
        // error still propagates, while an absent object ends only this run's
        // cleanup.
        let Some(retained) = store.read_text_versioned(&path).await? else {
            continue;
        };
        let retained: Value = serde_json::from_str(&retained.content)?;
        crate::queue::submit::validate_stored_run_manifest(&retained, &run_id)
            .map_err(|error| StorageError::Other(error.to_string()))?;

        summary.deleted_jobs += sweep_retained_run(store, &run_id, &job_ids).await?;
        summary.reaped_runs += 1;
        touched += 1;
        if limit > 0 && touched >= limit {
            break;
        }
    }
    Ok(summary)
}
