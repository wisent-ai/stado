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
    let mut deleted = 0;
    for job_id in job_ids {
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
        delete_status_dir(store, job_id).await?;
    }
    let path = format!("{RUN_PREFIX}/{run_id}.json");
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
    for run_id in list_runs(store).await? {
        if read_run(store, &run_id).await?.is_none() {
            continue;
        }
        let path = format!("{RUN_PREFIX}/{run_id}.json");
        let Some(initial) = store.read_text_versioned(&path).await? else {
            continue;
        };
        let initial_manifest: Value = serde_json::from_str(&initial.content)?;
        crate::queue::submit::validate_stored_run_manifest(&initial_manifest, &run_id)
            .map_err(|error| StorageError::Other(error.to_string()))?;
        if initial_manifest
            .get(CLEANUP_COMPLETED_AT)
            .is_some_and(py_truthy)
        {
            continue;
        }
        if initial_manifest.get(REAPED_AT).is_some_and(py_truthy) {
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
        if initial_manifest.get("schema").and_then(Value::as_str) != Some("stado.run-submission.v3")
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
            record_terminal_outcome(store, &job, prefix).await?;
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
        let retained = read_run(store, &run_id).await?.ok_or_else(|| {
            StorageError::Other(format!(
                "run manifest {run_id} disappeared after reaping CAS"
            ))
        })?;
        crate::queue::submit::validate_stored_run_manifest(&Value::Object(retained), &run_id)
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
