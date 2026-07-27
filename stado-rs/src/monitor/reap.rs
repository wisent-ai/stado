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
        let Some(mut manifest) = read_run(store, &run_id).await? else { continue };
        if manifest.get("reaped_at").is_some_and(py_truthy) {
            continue;
        }
        summary.examined_runs += 1;
        let Some(status) = run_status(store, &run_id).await? else { continue };
        if !status.all_terminal {
            continue;
        }

        // Python manifest["job_ids"] — a missing/non-array key raises there.
        let job_ids: Vec<String> = match manifest.get("job_ids").and_then(Value::as_array) {
            Some(arr) => arr.iter().filter_map(|j| j.as_str().map(str::to_string)).collect(),
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::Job;
    use crate::queue::local_file::LocalBackend;
    use crate::queue::runs::{write_run_manifest, RunManifest};
    use std::sync::Arc;

    fn store() -> (tempfile::TempDir, JobStorage) {
        let dir = tempfile::tempdir().unwrap();
        let backend = LocalBackend::new(dir.path().to_str().unwrap()).unwrap();
        (dir, JobStorage::with_backend(Arc::new(backend), "local"))
    }

    async fn write_run(store: &JobStorage, run_id: &str, job_ids: &[&str]) {
        let commands: Vec<String> = job_ids.iter().map(|j| format!("echo {j}")).collect();
        let ids: Vec<String> = job_ids.iter().map(|s| s.to_string()).collect();
        write_run_manifest(
            store,
            &RunManifest {
                run_id,
                name: None,
                submitter_app: None,
                submitted_by: "tester",
                submitted_from: "localhost",
                commands: &commands,
                job_ids: &ids,
            },
        )
        .await
        .unwrap();
    }

    fn job(job_id: &str) -> Job {
        Job::new(job_id, format!("echo {job_id}"))
    }

    #[tokio::test]
    async fn reaps_only_fully_terminal_runs() {
        let (_dir, store) = store();
        // Run A: fully terminal (one completed, one failed).
        write_run(&store, "run-a", &["ja1", "ja2"]).await;
        store.write_job("completed", &job("ja1")).await.unwrap();
        store.write_job("failed", &job("ja2")).await.unwrap();
        store.upload_text("status/ja1/heartbeat", "RUNNING 2026-05-13T00:26:33Z").await.unwrap();
        store.upload_text("status/ja1/status", "COMPLETED").await.unwrap();
        store.upload_text("status/ja2/status", "FAILED").await.unwrap();
        // Run B: still has a job in running/.
        write_run(&store, "run-b", &["jb1"]).await;
        store.write_job("running", &job("jb1")).await.unwrap();

        let summary = reap_terminal_runs(&store, 0).await.unwrap();
        assert_eq!(summary, ReapSummary { reaped_runs: 1, deleted_jobs: 2, examined_runs: 2 });

        // Run A manifest snapshotted; per-job blobs + status dirs gone.
        let manifest = read_run(&store, "run-a").await.unwrap().unwrap();
        assert!(manifest["reaped_at"].as_str().unwrap().ends_with("+00:00"));
        assert_eq!(manifest["final_counts"]["completed"], Value::from(1));
        assert_eq!(manifest["final_counts"]["failed"], Value::from(1));
        assert!(store.read_job("completed", "ja1").await.unwrap().is_none());
        assert!(store.read_job("failed", "ja2").await.unwrap().is_none());
        assert!(store.list_paths("status/ja1/", 0).await.unwrap().is_empty());
        assert!(store.list_paths("status/ja2/", 0).await.unwrap().is_empty());

        // Run B untouched.
        let manifest_b = read_run(&store, "run-b").await.unwrap().unwrap();
        assert!(manifest_b.get("reaped_at").is_none());
        assert!(store.read_job("running", "jb1").await.unwrap().is_some());

        // A second call skips the already-reaped manifest; run-b is
        // examined again (no reaped_at) but is still not terminal.
        let second = reap_terminal_runs(&store, 0).await.unwrap();
        assert_eq!(second, ReapSummary { reaped_runs: 0, deleted_jobs: 0, examined_runs: 1 });
    }

    #[tokio::test]
    async fn limit_caps_reaped_runs_per_tick() {
        let (_dir, store) = store();
        for run_id in ["run-c1", "run-c2"] {
            let jid = format!("j{run_id}");
            write_run(&store, run_id, &[&jid]).await;
            store.write_job("completed", &job(&jid)).await.unwrap();
        }

        let summary = reap_terminal_runs(&store, 1).await.unwrap();
        assert_eq!(summary.reaped_runs, 1);
        assert_eq!(summary.deleted_jobs, 1);
        // The cap breaks right after the first reap, so the second run is
        // never even examined (Python parity).
        assert_eq!(summary.examined_runs, 1);

        // Exactly one manifest got reaped_at; the other is still pending.
        let m1 = read_run(&store, "run-c1").await.unwrap().unwrap();
        let m2 = read_run(&store, "run-c2").await.unwrap().unwrap();
        let reaped = [m1.get("reaped_at").is_some(), m2.get("reaped_at").is_some()];
        assert_eq!(reaped.iter().filter(|b| **b).count(), 1);
    }
}
