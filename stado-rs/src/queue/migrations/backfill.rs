//! The bounded sweep itself: the marker names already in the index, and the
//! resumable pass over `queue/` that writes whatever is missing.

use std::collections::HashSet;

use futures::StreamExt;

use crate::models::Job;
use crate::queue::storage::JobStorage;
use crate::queue::{listing, StorageError};

use super::budgets::{DOWNLOAD_WORKERS, MARKER_PRUNE_PER_CALL};
use super::prune::prune_stale_markers;
use super::sentinel::{read_sentinel, write_sentinel};

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
///
/// [`super::listing::marker_path`]: crate::queue::listing::marker_path
/// [`super::listing::is_marker`]: crate::queue::listing::is_marker
async fn existing_marker_names(store: &JobStorage) -> Result<HashSet<String>, StorageError> {
    let mut out = HashSet::new();
    for path in store.list_paths(listing::MARKER_PREFIX, 0).await? {
        if listing::is_marker(&path) {
            out.insert(path);
        }
    }
    Ok(out)
}

/// Scan queue/ in bounded batches and write any missing marker. Returns the
/// same coverage answer [`has_swept`] reads, so a caller that already ran a
/// sweep needs no second read.
///
/// NOT cheap: two whole-prefix name listings plus up to `batch` job
/// documents. Callers that only need to know whether the index is covered
/// MUST ask [`has_swept`]; this is the repair, and it belongs on a tick, not
/// on a scheduler poll.
///
/// This is the bounded repair that keeps an unindexed job reachable, and it
/// is the ONLY one: the widening from priority>0 to every queued job extends
/// this pass rather than adding a second mechanism beside it.
///
/// It never stops. `done` used to latch terminally, which was right while the
/// index was an optimization and wrong the moment it became the only way to
/// see a queued job: a marker lost after the sweep completed — a failed write
/// during plain admission, a process killed between the queue blob and its
/// marker — left that job invisible to every scheduler forever, because
/// nothing would ever look again. So `done` now records only "swept once, the
/// whole-prefix pass can be switched off", and the cursor REWINDS to the head
/// instead of latching, so every later call keeps repairing a bounded batch.
/// The per-call cost stays a names-only listing plus at most `batch` bodies.
///
/// [`has_swept`]: super::has_swept
pub async fn backfill_priority_markers(
    store: &JobStorage,
    batch: usize,
) -> Result<bool, StorageError> {
    let state = read_sentinel(store).await?;
    // Marker names FIRST, queue names second, and the order is load-bearing:
    // see `prune_stale_markers` for the race it closes.
    let have = existing_marker_names(store).await?;
    let mut paths: Vec<String> = store
        .list_paths("queue/", 0)
        .await?
        .into_iter()
        .filter(|p| p.ends_with(".json"))
        .collect();
    paths.sort();
    let queued_ids: HashSet<String> = paths
        .iter()
        .filter_map(|path| {
            path.rsplit('/')
                .next()
                .and_then(|name| name.strip_suffix(".json"))
                .map(str::to_string)
        })
        .collect();
    if !state.cursor.is_empty() {
        // Python `paths[bisect_right(paths, cursor):]`.
        let cut = paths.partition_point(|p| p.as_str() <= state.cursor.as_str());
        paths.drain(..cut);
    }
    if paths.is_empty() {
        // End of a sweep. Record that one completed and rewind to the head so
        // the next call starts over rather than never running again.
        prune_stale_markers(store, &have, &queued_ids).await?;
        write_sentinel(store, "", true).await?;
        return Ok(true);
    }
    let chunk: Vec<String> = paths.into_iter().take(batch).collect();
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
        let current = listing::marker_path(&job);
        if !have.contains(&current) {
            store.write_priority_marker(&job).await?;
        }
        // And drop this job's SUPERSEDED entries. A queued job keeps exactly
        // one index position; the others are names left behind by an earlier
        // `created_at` or priority, and they are the bulk of the bloat that
        // stalled claiming — five queued jobs held about 1,325 markers each.
        // `prune_stale_markers` cannot see them, because they name a job that
        // IS queued; only the job's own current key distinguishes them, and
        // that key is derivable only here, where the body is in hand.
        let suffix = format!("-{}.json", job.job_id);
        let superseded: Vec<&String> = have
            .iter()
            .filter(|path| **path != current && path.ends_with(&suffix))
            .collect();
        for path in superseded.into_iter().take(MARKER_PRUNE_PER_CALL) {
            store.delete_blob(path).await?;
        }
    }
    prune_stale_markers(store, &have, &queued_ids).await?;
    // A chunk short of `batch` is the tail of the prefix: this sweep reached
    // the end. `done` is sticky — once any sweep has covered the prefix, the
    // whole-prefix pass stays retired even while a later sweep is mid-flight —
    // and the cursor keeps advancing so the repair itself never stops.
    let new_cursor = chunk.last().cloned().unwrap_or_default();
    let swept = state.done || chunk.len() < batch;
    let next_cursor = if chunk.len() < batch { "" } else { &new_cursor };
    write_sentinel(store, next_cursor, swept).await?;
    Ok(swept)
}
