//! The two durable receipts a reap writes, how they are written and read,
//! and the retained-outcome contract they stand for.

use chrono::{DateTime, SecondsFormat, Utc};
use serde_json::Value;

use crate::queue::runs::TERMINAL_PREFIXES;
use crate::queue::StorageError;

/// The time a reap writes into a receipt: RFC 3339 at whole seconds.
pub(super) fn stamp() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Secs, false)
}

/// Whether the reap wrote `marker` into `run_id`'s manifest. Only an absent
/// key is unwritten and only a time is written: Python truthiness read any
/// non-empty string, number or object as a finished cleanup, so a manifest
/// carrying something else is refused naming the key and what it holds.
pub(super) fn marked(manifest: &Value, marker: &str, run_id: &str) -> Result<bool, StorageError> {
    match manifest.get(marker) {
        None => Ok(false),
        Some(Value::String(at)) if DateTime::parse_from_rfc3339(at).is_ok() => Ok(true),
        Some(other) => Err(StorageError::Other(format!(
            "run manifest {run_id} carries {marker} as {other}, not the time a reap wrote it"
        ))),
    }
}

/// Manifest key: the outcomes of every entry are durably retained.
pub(super) const REAPED_AT: &str = "reaped_at";
/// Manifest key: every lifecycle blob and status entry of a retained run has
/// been deleted. Written only after that deletion succeeded in full.
pub(super) const CLEANUP_COMPLETED_AT: &str = "cleanup_completed_at";

/// Job ids of a manifest's durable entries.
pub(super) fn manifest_job_ids(
    manifest: &Value,
    run_id: &str,
) -> Result<Vec<String>, StorageError> {
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
pub(super) fn has_complete_retained_outcomes(manifest: &Value) -> bool {
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
