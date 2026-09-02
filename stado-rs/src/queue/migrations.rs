//! Idempotent priority-marker backfill for pre-0.4.26 queued jobs.
//!
//! Port of `stado/queue/migrations.py`.
//!
//! The priority-marker index introduced in 0.4.26 is auto-populated by
//! durable queue admission. Jobs already sitting in queue/
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

/// The coverage a completed sentinel attests to.
///
/// The backfill used to mean "every queued job with priority>0 has a marker",
/// and a store that finished it recorded `done` forever. The index now has to
/// name EVERY queued job, so that recorded `done` is an answer to a question
/// nobody is asking any more: honouring it would leave every pre-existing
/// priority-0 job unindexed and, since the listing walk is the index,
/// invisible. Stamping the coverage into the sentinel makes a `done` from the
/// narrower era simply not apply, so the pass runs again — once — and then
/// reports complete under the new coverage.
const COVERAGE: &str = "all-queued";

/// Python sentinel dict `{"cursor": str, "done": bool}`, plus the coverage
/// the `done` belongs to.
struct Sentinel {
    cursor: String,
    done: bool,
}

/// Python `_read_sentinel`, with the coverage check folded in.
///
/// A sentinel written before the index covered every queued job carries no
/// `coverage` key; its `done` and its `cursor` both describe the narrower
/// pass, so neither is usable and the walk restarts from the head under the
/// current coverage.
async fn read_sentinel(store: &JobStorage) -> Result<Sentinel, StorageError> {
    let fresh = Sentinel {
        cursor: String::new(),
        done: false,
    };
    let Some(raw) = store.download_text(SENTINEL_PATH).await? else {
        return Ok(fresh);
    };
    if raw.is_empty() {
        return Ok(fresh);
    }
    let value: serde_json::Value = serde_json::from_str(&raw)?;
    if value.get("coverage").and_then(serde_json::Value::as_str) != Some(COVERAGE) {
        return Ok(fresh);
    }
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
/// with default separators), stamped with the coverage its `done` attests to.
async fn write_sentinel(store: &JobStorage, cursor: &str, done: bool) -> Result<(), StorageError> {
    let body = super::python_json_dumps(
        &serde_json::json!({"cursor": cursor, "done": done, "coverage": COVERAGE}),
    )?;
    store.upload_text(SENTINEL_PATH, &body).await
}

/// Every marker name that already exists, replacing Python
/// `_existing_marker_job_ids`.
///
/// Names, not job_ids, because the name is what the backfill can compute: it
/// holds the Job, so [`super::listing::marker_path`] tells it exactly which
/// object should exist. Recovering a job_id from a name is not possible
/// anyway (see [`super::listing::is_marker`]) — the old parse took the
/// segment after the last `-` and so never matched a real id, which made
/// every pass rewrite every marker it had just confirmed.
///
/// Comparing names also catches the case comparing ids could not: a marker
/// sitting under a superseded key is not the marker this job needs, so the
/// current one still gets written.
async fn existing_marker_names(store: &JobStorage) -> Result<HashSet<String>, StorageError> {
    let mut out = HashSet::new();
    for path in store.list_paths(super::listing::MARKER_PREFIX, 0).await? {
        if super::listing::is_marker(&path) {
            out.insert(path);
        }
    }
    Ok(out)
}

/// Scan queue/ and write missing markers for queued jobs. Returns `true` iff
/// migration is complete (sentinel.done set). Idempotent: safe to call
/// repeatedly. Each call processes at most `batch` queue/ blobs.
///
/// This is the bounded repair that keeps an unindexed job reachable, and it
/// is the ONLY one: the widening from priority>0 to every queued job extends
/// this pass rather than adding a second mechanism beside it.
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
    let have = existing_marker_names(store).await?;
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
        // Queued-only, deliberately: a marker must never be resurrected for a
        // job that has already left the queue, or the walk would resolve it
        // against a `queue/` blob that no longer exists forever. The priority
        // floor that used to sit beside this check is gone — the index covers
        // every queued job now, and `priority_key` orders priority 0 as
        // correctly as any other value.
        if job.state != crate::models::job_state::QUEUED {
            continue;
        }
        if have.contains(&super::listing::marker_path(&job)) {
            continue;
        }
        store.write_priority_marker(&job).await?;
    }
    let new_cursor = chunk.last().cloned().unwrap_or_default();
    let is_done = chunk.len() < batch;
    write_sentinel(store, &new_cursor, is_done).await?;
    Ok(is_done)
}
