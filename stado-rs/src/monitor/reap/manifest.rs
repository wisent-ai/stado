//! The two durable receipts a reap writes, the Python truthiness they are
//! tested with, and the retained-outcome contract they stand for.

use serde_json::Value;

use crate::queue::runs::TERMINAL_PREFIXES;
use crate::queue::StorageError;

/// Python truthiness for the `manifest.get("reaped_at")` skip check.
pub(super) fn py_truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().is_some_and(|f| f != 0.0),
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
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
