//! Standing priority-marker repair for queued jobs missing an index entry.
//!
//! Port of `stado/queue/migrations.py`, widened past its original job.
//!
//! The priority-marker index introduced in 0.4.26 is populated by durable
//! queue admission. Jobs already sitting in queue/ when 0.4.26 deployed have
//! no marker, and so does any job whose marker write failed to land after it.
//! Either way the job is stranded: the listing walk IS the index, so an
//! unindexed queued job is never claimed while it still reports `queued`.
//!
//! This module fixes that continuously. The standing bounded repair,
//! [`backfill_priority_markers`], is driven from the coordinator tick by
//! [`crate::queue::reaper::reap_expired_leases`]. It is also driven from
//! [`super::listing::list_claimable`], but ONLY while the whole-prefix
//! fallback is still live: a poll asks the cheap [`has_swept`] first, so a
//! store whose index is already covered pays one small read per poll rather
//! than a sweep. It is resumable via a queue_priority/.migration.json
//! sentinel recording (cursor, done, coverage). Each call processes at most
//! `batch` blobs, to fit comfortably inside a Cloud Function 60s tick, and
//! resumes past the recorded cursor.
//!
//! The cursor WRAPS: reaching the end of queue/ rewinds it to the head so
//! the next call sweeps again. `done` no longer terminates the pass — it
//! only records that one full sweep has happened, which retires the
//! whole-prefix listing fallback. A repair that latched shut could not see a
//! marker lost after it completed, and nothing else would ever look.

use std::collections::HashSet;

use futures::StreamExt;

use crate::models::Job;

use super::storage::JobStorage;
use super::StorageError;

/// Python `_SENTINEL_PATH`.
pub const SENTINEL_PATH: &str = "queue_priority/.migration.json";
/// Python `BACKFILL_BATCH`.
pub const BACKFILL_BATCH: usize = 500;
/// Index entries one repair call may delete.
///
/// The same shape of bound as `BACKFILL_BATCH` and for the same reason: the
/// repair runs on a tick, and replacing an unbounded read cost with an
/// unbounded delete cost would be no improvement. At this size the 9,021
/// markers measured on 2026-09-03 clear in a handful of ticks.
pub const MARKER_PRUNE_PER_CALL: usize = 500;
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
///
/// `done` no longer means "this migration is over and must never run again".
/// It means "the sweep has covered the prefix end-to-end at least once", so
/// the whole-prefix fallback in [`super::listing::list_claimable`] can be
/// switched off. It is sticky across the cursor's rewind: the sweep keeps
/// running forever, the fallback stays retired.
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

/// Whether some sweep has covered `queue/` end-to-end under the current
/// coverage. One small download, and the only question
/// [`super::listing::list_claimable`] needs answered per poll.
///
/// Deliberately separate from [`backfill_priority_markers`]: coverage is
/// cheap to read, the repair that establishes it is not, and conflating them
/// made every poll pay a sweep. A sentinel from the narrower priority>0 era
/// reads as not-swept, exactly as the walk requires.
pub async fn has_swept(store: &JobStorage) -> Result<bool, StorageError> {
    Ok(read_sentinel(store).await?.done)
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
/// fallback can be switched off", and the cursor REWINDS to the head instead
/// of latching, so every later call keeps repairing a bounded batch. The
/// per-call cost stays a names-only listing plus at most `batch` bodies.
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
        let current = super::listing::marker_path(&job);
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
    // fallback stays retired even while a later sweep is mid-flight — and the
    // cursor keeps advancing so the repair itself never stops.
    let new_cursor = chunk.last().cloned().unwrap_or_default();
    let swept = state.done || chunk.len() < batch;
    let next_cursor = if chunk.len() < batch { "" } else { &new_cursor };
    write_sentinel(store, next_cursor, swept).await?;
    Ok(swept)
}

/// Delete index entries that name no queued job, bounded per call.
///
/// # The defect this exists for
///
/// The repair above only ever ADDED. Nothing pruned, and a marker name is
/// `<inv_priority>-<created_at>-<job_id>`, so the same job under a new
/// `created_at` — a requeue, a re-admission, any rewrite of the queue blob —
/// produces a NEW object while the old one stays forever. Measured on the
/// fleet store on 2026-09-03: 9,021 markers naming 161 distinct job ids, five
/// of those ids holding about 1,325 markers each, against twelve jobs
/// actually queued.
///
/// That is not untidiness, it is the second half of an eleven-day queue
/// stall. [`super::listing::list_claimable`] walks this index and charges one
/// unit of scan budget per marker whether or not a job comes back, so a claim
/// poll paid up to 8,000 marker reads plus the job reads behind them before it
/// could see anything claimable — minutes to hours on this store, on every
/// poll, and the cursor only advances if the walk finishes. Hosts with clean
/// gates and free slots claimed nothing.
///
/// # Why deleting here is safe
///
/// A marker's job id IS recoverable by suffix even though it is not
/// recoverable by splitting (see [`super::listing::is_marker`]): job ids are
/// fixed-shape, so `-<job_id>.json` matches exactly one id, which is the same
/// test [`super::listing::delete_markers_scanning`] already trusts for
/// removal. The absence check is a name listing, never a body read: a marker
/// whose id is not among the `queue/` blob names names no queued job.
///
/// The listing ORDER closes the only race. Markers are listed BEFORE the
/// queue blobs, so a job admitted concurrently either had its marker written
/// before the marker listing — in which case its blob is in the later queue
/// listing and it is kept — or its marker is not in `have` at all and is
/// therefore never a deletion candidate. A pruned marker for a job that is
/// still queued cannot result; and were one ever lost, this same repair
/// rewrites it on the next sweep, which is the property it was given when
/// `done` stopped latching.
///
/// Bounded like every other half of this pass: markers are derived data, and
/// a repair that deleted thousands of objects in one call would replace one
/// unbounded per-tick cost with another.
async fn prune_stale_markers(
    store: &JobStorage,
    have: &HashSet<String>,
    queued_ids: &HashSet<String>,
) -> Result<usize, StorageError> {
    let live_names: HashSet<String> = queued_ids
        .iter()
        .map(|id| format!("-{id}.json"))
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();
    let mut stale: Vec<&String> = have
        .iter()
        .filter(|path| {
            *path != SENTINEL_PATH
                && super::listing::is_marker(path)
                && !live_names.iter().any(|suffix| path.ends_with(suffix))
        })
        .collect();
    // Oldest index positions first, so the pruning walks the same order the
    // claim walk pays for and the head of the index clears first.
    stale.sort();
    let mut removed = 0usize;
    for path in stale.into_iter().take(MARKER_PRUNE_PER_CALL) {
        store.delete_blob(path).await?;
        removed += 1;
    }
    Ok(removed)
}
