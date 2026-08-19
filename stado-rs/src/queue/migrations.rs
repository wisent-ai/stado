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

/// The same bulk fan-out under a crate-visible name, so `queue::copy` can
/// reuse this budget for its backend-to-backend pass instead of picking a
/// second concurrency number.
pub(crate) const BULK_WORKERS: usize = DOWNLOAD_WORKERS;

/// Python sentinel dict `{"cursor": str, "done": bool}`.
struct Sentinel {
    cursor: String,
    done: bool,
}

/// Python `_read_sentinel`.
async fn read_sentinel(store: &JobStorage) -> Result<Sentinel, StorageError> {
    let Some(raw) = store.download_text(SENTINEL_PATH).await? else {
        return Ok(Sentinel {
            cursor: String::new(),
            done: false,
        });
    };
    if raw.is_empty() {
        return Ok(Sentinel {
            cursor: String::new(),
            done: false,
        });
    }
    let value: serde_json::Value = serde_json::from_str(&raw)?;
    Ok(Sentinel {
        cursor: value
            .get("cursor")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("")
            .to_string(),
        done: value
            .get("done")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false),
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
