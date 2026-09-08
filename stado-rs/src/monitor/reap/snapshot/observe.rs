//! Re-observation of a completed reconciliation: every decision the sealed
//! snapshot produced is read back through the contract that made it.

use serde_json::Value;

use crate::monitor::reap::manifest::{
    has_complete_retained_outcomes, py_truthy, CLEANUP_COMPLETED_AT,
};
use crate::monitor::reap::reads::required_snapshot_text;
use crate::queue::runs::TERMINAL_PREFIXES;
use crate::queue::{JobStorage, StorageError};

use super::retained::{prove_snapshot_content_retained, reconciliation_store_path};

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
