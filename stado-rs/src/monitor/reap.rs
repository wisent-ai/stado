//! By-run reaper: removes per-job cruft once a run is fully terminal.
//!
//! Port of `stado/monitor/reap/run_reaper.py`.
//!
//! A run is reapable when none of its member jobs are in queue/ or running/.
//! On reap we snapshot the final completed/failed counts into the run
//! manifest (a single-writer mutation — only the reaper does this, so no
//! fleet contention), then delete the heavy per-job blobs and their status
//! dirs. The lightweight run manifest is kept as the permanent record, so
//! the queue stops accumulating thousands of orphaned per-job blobs.

use chrono::Utc;
use serde_json::{Map, Value};

use crate::queue::runs::{list_runs, read_run, run_status, ALL_PREFIXES, RUN_PREFIX};
use crate::queue::{JobStorage, StorageError};

/// NOTE: only these two — Python run_reaper.py's local TERMINAL_PREFIXES,
/// deliberately NOT queue::runs::TERMINAL_PREFIXES (which also covers
/// uploaded/ and cancelled/).
const TERMINAL_PREFIXES: [&str; 2] = ["completed", "failed"];

/// Python summary dict {"reaped_runs", "deleted_jobs", "examined_runs"}.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReapSummary {
    pub reaped_runs: i64,
    pub deleted_jobs: i64,
    pub examined_runs: i64,
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

/// Delete every blob under status/<job_id>/.
async fn delete_status_dir(store: &JobStorage, job_id: &str) -> Result<(), StorageError> {
    for path in store.list_paths(&format!("status/{job_id}/"), 0).await? {
        store.delete_blob(&path).await?;
    }
    Ok(())
}

/// Reap all fully-terminal runs. Returns a summary.
///
/// `limit > 0` caps how many runs are reaped this tick (bounds per-tick
/// work on a large backlog); 0 means no cap.
pub async fn reap_terminal_runs(
    store: &JobStorage,
    limit: i64,
) -> Result<ReapSummary, StorageError> {
    let mut summary = ReapSummary::default();
    for run_id in list_runs(store).await? {
        let Some(mut manifest) = read_run(store, &run_id).await? else {
            continue;
        };
        if manifest.get("reaped_at").is_some_and(py_truthy) {
            continue;
        }
        summary.examined_runs += 1;
        let Some(status) = run_status(store, &run_id).await? else {
            continue;
        };
        if !status.all_terminal {
            continue;
        }

        // Python manifest["job_ids"] — a missing/non-array key raises there.
        let job_ids: Vec<String> = match manifest.get("job_ids").and_then(Value::as_array) {
            Some(arr) => arr
                .iter()
                .filter_map(|j| j.as_str().map(str::to_string))
                .collect(),
            None => {
                return Err(StorageError::Other(format!(
                    "run manifest {run_id} missing job_ids"
                )));
            }
        };

        // Snapshot the outcome before the per-job blobs disappear.
        manifest.insert(
            "reaped_at".into(),
            Value::from(Utc::now().format("%Y-%m-%dT%H:%M:%S+00:00").to_string()),
        );
        // Python counts = {p: 0 for p in ALL_PREFIXES}; keep the same key
        // insertion order in the JSON snapshot.
        let counts: Map<String, Value> = ALL_PREFIXES
            .iter()
            .map(|p| (p.to_string(), Value::from(status.counts[*p])))
            .collect();
        manifest.insert("final_counts".into(), Value::Object(counts));
        store
            .upload_text(
                &format!("{RUN_PREFIX}/{run_id}.json"),
                &serde_json::to_string_pretty(&Value::Object(manifest))?,
            )
            .await?;

        for jid in &job_ids {
            for prefix in TERMINAL_PREFIXES {
                if store.read_job(prefix, jid).await?.is_some() {
                    store.delete_job(prefix, jid).await?;
                    summary.deleted_jobs += 1;
                }
            }
            delete_status_dir(store, jid).await?;
        }

        summary.reaped_runs += 1;
        if limit > 0 && summary.reaped_runs >= limit {
            break;
        }
    }
    Ok(summary)
}

