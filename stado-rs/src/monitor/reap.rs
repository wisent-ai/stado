//! By-run reaper: removes per-job cruft once a run is fully terminal.
//!
//! Port of `stado/monitor/reap/run_reaper.py`.
//!
//! A run is reapable when every durable entry has reached a terminal prefix.
//! Reaping first CAS-retains each exact terminal job in the manifest and marks
//! entries reaped; only that committed snapshot permits lifecycle blob deletion.

use chrono::Utc;
use serde_json::{Map, Value};

use crate::queue::runs::{
    list_runs, read_run, record_terminal_outcome, run_status, ALL_PREFIXES, RUN_PREFIX,
    TERMINAL_PREFIXES,
};
use crate::queue::{JobStorage, StorageError};

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
        if read_run(store, &run_id).await?.is_none() {
            continue;
        }
        let path = format!("{RUN_PREFIX}/{run_id}.json");
        let Some(initial) = store.read_text_versioned(&path).await? else {
            continue;
        };
        let initial_manifest: Value = serde_json::from_str(&initial.content)?;
        if initial_manifest.get("reaped_at").is_some_and(py_truthy) {
            continue;
        }
        if initial_manifest.get("schema").and_then(Value::as_str)
            != Some("stado.run-submission.v3")
        {
            return Err(StorageError::Other(format!(
                "run manifest {run_id} requires explicit durable-entry migration before reaping"
            )));
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
        for entry in entries {
            if entry.get("outcome").is_some_and(Value::is_object) {
                continue;
            }
            let job_id = entry
                .get("job_id")
                .and_then(Value::as_str)
                .ok_or_else(|| {
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
            record_terminal_outcome(store, &job, prefix).await?;
        }

        let Some(versioned) = store.read_text_versioned(&path).await? else {
            continue;
        };
        let mut manifest: Value = serde_json::from_str(&versioned.content)?;
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
            entry.insert("reaped_at".into(), Value::from(reaped_at.as_str()));
        }
        let manifest_object = manifest
            .as_object_mut()
            .expect("validated run manifest object");
        manifest_object.insert("reaped_at".into(), Value::from(reaped_at));
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

        for job_id in &job_ids {
            for prefix in TERMINAL_PREFIXES {
                if store.read_job(prefix, job_id).await?.is_some() {
                    store.delete_job(prefix, job_id).await?;
                    summary.deleted_jobs += 1;
                }
            }
            for source_prefix in ["queue", "running"] {
                if store.read_job(source_prefix, job_id).await?.is_some() {
                    store.delete_job(source_prefix, job_id).await?;
                    summary.deleted_jobs += 1;
                }
            }
            delete_status_dir(store, job_id).await?;
        }
        summary.reaped_runs += 1;
        if limit > 0 && summary.reaped_runs >= limit {
            break;
        }
    }
    Ok(summary)
}
