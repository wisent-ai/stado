//! Idempotent priority-marker backfill for pre-0.4.26 queued jobs.
//!
//! Port of `stado/queue/migrations.py`.
//!
//! The priority-marker index introduced in 0.4.26 is auto-populated by
//! JobStorage.write_job() at submit time. Jobs already sitting in queue/
//! when 0.4.26 deployed have no marker and would remain stranded behind
//! the FIFO listing window because the scheduler only loads the oldest N
//! queue/ blobs by GCS time_created.
//!
//! This module fixes that automatically: list_jobs_priority_first calls
//! backfill_priority_markers() before doing its priority-listing pass.
//! The backfill is resumable via a queue_priority/.migration.json sentinel
//! that records (cursor, done). Each call processes BACKFILL_BATCH blobs
//! to fit comfortably inside a Cloud Function 60s tick; subsequent calls
//! resume past the recorded cursor. Once cursor reaches the end of queue/,
//! sentinel.done=True and every future call returns immediately.

use std::collections::HashSet;

use futures::StreamExt;

use crate::models::Job;

use super::storage::JobStorage;
use super::StorageError;

/// Python `_SENTINEL_PATH`.
pub const SENTINEL_PATH: &str = "queue_priority/.migration.json";
/// Python `BACKFILL_BATCH`.
pub const BACKFILL_BATCH: usize = 500;
/// Python `_DOWNLOAD_WORKERS`.
const DOWNLOAD_WORKERS: usize = 10;

/// Python sentinel dict `{"cursor": str, "done": bool}`.
struct Sentinel {
    cursor: String,
    done: bool,
}

/// Python `_read_sentinel`.
async fn read_sentinel(store: &JobStorage) -> Result<Sentinel, StorageError> {
    let Some(raw) = store.download_text(SENTINEL_PATH).await? else {
        return Ok(Sentinel { cursor: String::new(), done: false });
    };
    if raw.is_empty() {
        return Ok(Sentinel { cursor: String::new(), done: false });
    }
    let value: serde_json::Value = serde_json::from_str(&raw)?;
    Ok(Sentinel {
        cursor: value
            .get("cursor")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("")
            .to_string(),
        done: value.get("done").and_then(serde_json::Value::as_bool).unwrap_or(false),
    })
}

/// Python `_write_sentinel` (`json.dumps({"cursor": ..., "done": ...})`
/// with default separators).
async fn write_sentinel(store: &JobStorage, cursor: &str, done: bool) -> Result<(), StorageError> {
    let body = super::python_json_dumps(&serde_json::json!({"cursor": cursor, "done": done}))?;
    store.upload_text(SENTINEL_PATH, &body).await
}

/// All job_ids that already have a priority marker. Each marker name ends
/// in `-{job_id}.json`. Python `_existing_marker_job_ids`.
async fn existing_marker_job_ids(store: &JobStorage) -> Result<HashSet<String>, StorageError> {
    let mut out = HashSet::new();
    for path in store.list_paths("queue_priority/", 0).await? {
        if !path.ends_with(".json") {
            continue;
        }
        // path layout: queue_priority/{inv}-{ts}-{job_id}.json (skip sentinel)
        let name = path.rsplit('/').next().unwrap_or("");
        if name.starts_with('.') {
            continue;
        }
        let tail = name.rsplit('-').next().unwrap_or("");
        out.insert(tail.strip_suffix(".json").unwrap_or(tail).to_string());
    }
    Ok(out)
}

/// Scan queue/ and write missing markers for priority>0 jobs. Returns
/// `true` iff migration is complete (sentinel.done set). Idempotent: safe
/// to call repeatedly. Each call processes at most `batch` queue/ blobs.
pub async fn backfill_priority_markers(
    store: &JobStorage,
    batch: usize,
) -> Result<bool, StorageError> {
    let state = read_sentinel(store).await?;
    if state.done {
        return Ok(true);
    }
    let mut paths: Vec<String> = store
        .list_paths("queue/", 0)
        .await?
        .into_iter()
        .filter(|p| p.ends_with(".json"))
        .collect();
    paths.sort();
    if !state.cursor.is_empty() {
        // Python `paths[bisect_right(paths, cursor):]`.
        let cut = paths.partition_point(|p| p.as_str() <= state.cursor.as_str());
        paths.drain(..cut);
    }
    if paths.is_empty() {
        write_sentinel(store, "", true).await?;
        return Ok(true);
    }
    let chunk: Vec<String> = paths.into_iter().take(batch).collect();
    let have = existing_marker_job_ids(store).await?;
    let bodies: Vec<Option<String>> = futures::stream::iter(&chunk)
        .map(|path| store.download_text(path))
        .buffered(DOWNLOAD_WORKERS)
        .collect::<Vec<Result<Option<String>, StorageError>>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()?;
    for body in bodies.into_iter().flatten() {
        if body.is_empty() {
            continue;
        }
        let job = Job::from_json(&body)?;
        if job.priority <= 0 {
            continue;
        }
        if have.contains(&job.job_id) {
            continue;
        }
        store.write_priority_marker(&job).await?;
    }
    let new_cursor = chunk.last().cloned().unwrap_or_default();
    let is_done = chunk.len() < batch;
    write_sentinel(store, &new_cursor, is_done).await?;
    Ok(is_done)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::queue::local_file::LocalBackend;
    use std::sync::Arc;

    fn store() -> (tempfile::TempDir, JobStorage) {
        let dir = tempfile::tempdir().unwrap();
        let backend = LocalBackend::new(dir.path().to_str().unwrap()).unwrap();
        (dir, JobStorage::with_backend(Arc::new(backend), "local"))
    }

    fn job(job_id: &str, priority: i64) -> Job {
        let mut job = Job::new(job_id, "echo hi");
        job.priority = priority;
        job.created_at = "2026-01-02T03:04:05+00:00".into();
        job
    }

    async fn sentinel(store: &JobStorage) -> serde_json::Value {
        store
            .download_text(SENTINEL_PATH)
            .await
            .unwrap()
            .map(|raw| serde_json::from_str(&raw).unwrap())
            .unwrap_or(serde_json::Value::Null)
    }

    async fn marker_job_ids(store: &JobStorage) -> Vec<String> {
        let mut ids: Vec<String> = store
            .list_paths("queue_priority/", 0)
            .await
            .unwrap()
            .iter()
            .filter_map(|p| {
                let name = p.rsplit('/').next().unwrap_or("");
                (!name.starts_with('.'))
                    .then(|| name.rsplit('-').next().unwrap_or("").replace(".json", ""))
            })
            .collect();
        ids.sort();
        ids
    }

    #[tokio::test]
    async fn backfill_is_resumable_across_runs_via_cursor() {
        let (_dir, store) = store();
        // j1 already has a marker (submitted post-0.4.26); j2..j5 predate
        // the index (uploaded raw, no marker/metadata); j6 has priority 0.
        store.write_job("queue", &job("j1", 7)).await.unwrap();
        for id in ["j2", "j3", "j4", "j5"] {
            store.upload_text(&format!("queue/{id}.json"), &job(id, 3).to_json()).await.unwrap();
        }
        store.upload_text("queue/j6.json", &job("j6", 0).to_json()).await.unwrap();

        // Run 1: bounded to batch=2 -> not done, cursor recorded, only the
        // first chunk processed (j1 already had a marker; j2 gets one).
        assert!(!backfill_priority_markers(&store, 2).await.unwrap());
        assert_eq!(
            sentinel(&store).await,
            serde_json::json!({"cursor": "queue/j2.json", "done": false})
        );
        assert_eq!(marker_job_ids(&store).await, vec!["j1", "j2"]);

        // Run 2 resumes past the cursor.
        assert!(!backfill_priority_markers(&store, 2).await.unwrap());
        assert_eq!(
            sentinel(&store).await,
            serde_json::json!({"cursor": "queue/j4.json", "done": false})
        );
        assert_eq!(marker_job_ids(&store).await, vec!["j1", "j2", "j3", "j4"]);

        // Run 3 consumes a full batch (j5, j6) -> still not done (len == batch).
        assert!(!backfill_priority_markers(&store, 2).await.unwrap());
        // Run 4 finds nothing past the cursor -> terminal done.
        assert!(backfill_priority_markers(&store, 2).await.unwrap());
        assert_eq!(sentinel(&store).await, serde_json::json!({"cursor": "", "done": true}));

        // j5 backfilled; priority-0 j6 never gets a marker.
        assert_eq!(marker_job_ids(&store).await, vec!["j1", "j2", "j3", "j4", "j5"]);

        // Once done, every future call returns immediately.
        assert!(backfill_priority_markers(&store, 2).await.unwrap());

        // The marker body matches the Python index entry shape.
        let markers = store.list_paths("queue_priority/", 0).await.unwrap();
        let j5_marker = markers.iter().find(|p| p.ends_with("-j5.json")).unwrap();
        assert_eq!(
            store.download_text(j5_marker).await.unwrap().as_deref(),
            Some("{\"job_id\": \"j5\", \"priority\": 3}")
        );
    }

    #[tokio::test]
    async fn backfill_on_empty_queue_completes_immediately() {
        let (_dir, store) = store();
        assert!(backfill_priority_markers(&store, BACKFILL_BATCH).await.unwrap());
        assert_eq!(sentinel(&store).await, serde_json::json!({"cursor": "", "done": true}));
        // Byte-compatible with Python json.dumps default separators.
        assert_eq!(
            store.download_text(SENTINEL_PATH).await.unwrap().as_deref(),
            Some("{\"cursor\": \"\", \"done\": true}")
        );
    }
}
