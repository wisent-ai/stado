//! Per-state counts for a run, probed from its members' current prefixes.

use std::collections::BTreeMap;

use serde_json::Value;

use crate::queue::storage::JobStorage;
use crate::queue::StorageError;

use super::super::prefixes::{ALL_PREFIXES, TERMINAL_PREFIXES};
use super::read::read_run;

/// Which prefix currently holds this job_id, or None if absent. Terminal wins
/// over transitional duplicates left by a crash during a fenced move.
async fn job_state(store: &JobStorage, job_id: &str) -> Result<Option<&'static str>, StorageError> {
    for prefix in [
        "cancelled",
        "failed",
        "uploaded",
        "completed",
        "running",
        "queue",
    ] {
        if store.read_job(prefix, job_id).await?.is_some() {
            return Ok(Some(prefix));
        }
    }
    Ok(None)
}

/// Per-state counts for a run, derived from its members' current prefixes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunStatus {
    pub run_id: String,
    pub submitter_app: String,
    pub n_jobs: i64,
    /// Count per prefix (all [`ALL_PREFIXES`](crate::queue::runs::ALL_PREFIXES) keys present).
    pub counts: BTreeMap<String, i64>,
    /// Member jobs present in no prefix.
    pub missing: i64,
    pub in_flight: i64,
    pub all_terminal: bool,
}

/// Derive per-state counts for a run; `None` if the manifest does not exist.
pub async fn run_status(
    store: &JobStorage,
    run_id: &str,
) -> Result<Option<RunStatus>, StorageError> {
    let Some(manifest) = read_run(store, run_id).await? else {
        return Ok(None);
    };
    crate::queue::submit::validate_stored_run_manifest(&Value::Object(manifest.clone()), run_id)
        .map_err(|error| StorageError::Other(error.to_string()))?;
    let entries = manifest
        .get("entries")
        .and_then(Value::as_array)
        .ok_or_else(|| {
            StorageError::Other(format!(
                "run manifest {run_id} has no durable entries; resubmit or explicitly migrate it"
            ))
        })?;
    let mut counts: BTreeMap<String, i64> =
        ALL_PREFIXES.iter().map(|p| (p.to_string(), 0)).collect();
    let mut missing = 0;
    for entry in entries {
        let job_id = entry.get("job_id").and_then(Value::as_str).ok_or_else(|| {
            StorageError::Other(format!("run manifest {run_id} has an invalid entry"))
        })?;
        let retained = entry
            .get("outcome")
            .and_then(Value::as_object)
            .and_then(|outcome| outcome.get("prefix"))
            .and_then(Value::as_str);
        match retained {
            Some(prefix) if TERMINAL_PREFIXES.contains(&prefix) => {
                *counts.get_mut(prefix).expect("terminal prefix initialized") += 1;
            }
            Some(prefix) => {
                return Err(StorageError::Other(format!(
                    "run manifest {run_id} retained invalid outcome prefix {prefix}"
                )));
            }
            None => match job_state(store, job_id).await? {
                Some(prefix) => *counts.get_mut(prefix).expect("prefix initialized") += 1,
                None => missing += 1,
            },
        }
    }
    let terminal: i64 = TERMINAL_PREFIXES.iter().map(|p| counts[*p]).sum();
    let in_flight = counts["queue"] + counts["running"];
    Ok(Some(RunStatus {
        run_id: run_id.to_string(),
        submitter_app: manifest
            .get("submitter_app")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        n_jobs: entries.len() as i64,
        counts,
        missing,
        in_flight,
        all_terminal: in_flight == 0 && missing == 0 && terminal > 0,
    }))
}
