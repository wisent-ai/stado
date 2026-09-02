//! Priority-marker index (`queue_priority/<inv_prio>-<ts>-<jid>.json`
//! markers so name-ascending sort = priority-desc + FIFO), the `list_jobs`
//! bulk fetch, the top-N priority listing, the priority-first listing, and
//! the metadata-prefiltered fitting-jobs listing.
//!
//! Port of `stado/queue/listing/__init__.py`.
//!
//! Known Python bug (ported as intended): `listing/__init__.py:120`,
//! `capacity.py:121` and `leases/__init__.py:143` reference
//! `store._azure_backend`, an attribute that never exists (the backend
//! handle is `_blob_backend`). The intended behavior is a single backend
//! handle — exactly what `JobStorage` carries here — so `list_fitting`
//! always takes the metadata-prefiltering path.
//!
//! # Index ordering rule
//!
//! **A queued job may never be observable without a marker. Whenever a write
//! and a delete both touch one job's index entries, the write goes first.**
//!
//! The two failure modes are not symmetric, and this asymmetry is what fixes
//! the order of every operation in this module:
//!
//! - A *duplicate* or *stale* marker is harmless. It resolves its job from
//!   its body, is deduplicated by job_id, and costs only scan budget — never
//!   a slot in the returned window.
//! - A *missing* marker is fatal. This index is the whole listing strategy
//!   for `queue/`, so an unindexed queued job is invisible to every
//!   scheduler forever while still reporting state `queued`: it is never
//!   claimed, and `queue drain --wait` never terminates.
//!
//! So: create writes the marker before settling the job blob; a re-key
//! writes the new marker before cleaning the superseded one (see
//! [`delete_markers_scanning`]'s `keep`); and the repair sweep in
//! [`migrations::backfill_priority_markers`] is bounded and repeating rather
//! than latched, so an interrupted window is always eventually re-swept.

use std::collections::HashSet;

use futures::StreamExt;

use crate::models::Job;

use super::storage::{is_transition_sentinel_state, JobStorage};
use super::{json_str, migrations, StorageError};

/// Sortable name component: lower = higher real priority + older.
///
/// Python `priority_key`: priority is clamped to 0..=99999999, inverted,
/// and zero-padded to 8 digits, followed by the ISO created_at.
pub fn priority_key(job: &Job) -> String {
    let prio = job.priority.clamp(0, 99_999_999);
    let inv = 99_999_999 - prio;
    format!("{inv:08}-{}", job.created_at)
}

/// The index prefix. Ordered by name, and the name is the ordering.
pub const MARKER_PREFIX: &str = "queue_priority/";

/// How many marker names one page of the ordered walk pulls.
///
/// Large enough that a full window is normally one listing round trip, small
/// enough that a scan which stops early has not paid for a page it will
/// never look at.
const MARKER_PAGE: usize = 256;

/// The marker name for `job`.
///
/// Deriving it from the job is what makes marker removal a single delete.
/// While it was only ever recovered by walking the index for a matching
/// suffix, every removal cost a listing of the whole index — tolerable while
/// the index held just the priority>0 jobs, a per-completion scan of the
/// entire queue now that it holds all of them.
pub fn marker_path(job: &Job) -> String {
    format!("{MARKER_PREFIX}{}-{}.json", priority_key(job), job.job_id)
}

/// Whether this path is a marker rather than the migration sentinel (or any
/// other bookkeeping object that shares the prefix).
///
/// Deliberately NOT a job_id parse. The name is
/// `<inv_priority>-<created_at>-<job_id>.json` and BOTH of the trailing
/// fields contain `-` of their own: `created_at` carries the date separators
/// (and a negative UTC offset would carry another), and a job_id looks like
/// `job-906b84bcaf55e7935aa9ba2d`. So there is no split position derivable
/// from the name alone — taking the segment after the last `-` yields
/// `906b84bcaf55e7935aa9ba2d`, an id that resolves to nothing. The marker
/// body states the job_id, and that is what readers use.
pub fn is_marker(path: &str) -> bool {
    path.rsplit('/')
        .next()
        .is_some_and(|name| !name.starts_with('.') && name.ends_with(".json"))
}

/// Index entry for a queued job.
pub async fn write_marker(store: &JobStorage, job: &Job) -> Result<(), StorageError> {
    // Python `json.dumps({"job_id": ..., "priority": int(...)})` with
    // default separators.
    let body = format!(
        "{{\"job_id\": {}, \"priority\": {}}}",
        json_str(&job.job_id),
        job.priority
    );
    store.upload_text(&marker_path(job), &body).await
}

/// Drop the marker this job names.
///
/// Exact, so it costs one delete. Correct because the name is a function of
/// `priority` and `created_at`, and a queued job's marker is rewritten
/// whenever its priority changes.
pub async fn delete_marker_for(store: &JobStorage, job: &Job) -> Result<(), StorageError> {
    store.delete_blob(&marker_path(job)).await
}

/// Repair path: drop every marker naming `job_id`, whatever key it was
/// written under, by walking the index — except `keep`, when the caller has
/// already written the marker the job needs.
///
/// This is the only remaining reason to traverse, and it exists for the two
/// cases where the name is genuinely not computable: a marker orphaned from a
/// job that no longer exists, and a marker left under a superseded key after
/// a priority change (the pre-change key cannot be derived from the
/// post-change job). Everything on a hot path uses [`delete_marker_for`]
/// instead.
///
/// `keep` exists because the safe order for a re-key is write-then-clean, and
/// this scan matches on job_id: without an exception it would delete the very
/// marker the caller just wrote and leave the job unindexed, which is the
/// failure this ordering is meant to avoid.
///
/// Matched on the `-<job_id>.json` suffix, which is exact here because job
/// ids are fixed-shape (`job-` + hex) and so no id can be a dash-delimited
/// suffix of another.
pub async fn delete_markers_scanning(
    store: &JobStorage,
    job_id: &str,
    keep: Option<&str>,
) -> Result<(), StorageError> {
    let suffix = format!("-{job_id}.json");
    for path in store.list_paths(MARKER_PREFIX, 0).await? {
        if is_marker(&path) && path.ends_with(&suffix) && Some(path.as_str()) != keep {
            store.delete_blob(&path).await?;
        }
    }
    Ok(())
}

/// Parallel-fetch job JSONs under `{prefix}/`.
///
/// `oldest_first > 0` caps the answer to that many jobs picked by creation
/// time — required for queue/ where 14k+ blobs would otherwise force the
/// scheduler to download every JSON before slicing per_tick_cap and blow the
/// 60s function timeout. Fetches fan out 10 ways.
///
/// The window is oldest-first, and the ordered path listing is taken EXACTLY
/// ONCE: the scheduler's FIFO fairness is "the N oldest queued blobs", and a
/// re-listing loop that widens a budget pays for the whole prefix again on
/// every round. Transitional sentinels are skipped without being counted, so
/// the walk simply continues down the already-ordered list until the window
/// holds N live jobs or the prefix is exhausted.
pub async fn list_jobs(
    store: &JobStorage,
    prefix: &str,
    oldest_first: usize,
) -> Result<Vec<Job>, StorageError> {
    let ordered = if oldest_first > 0 { usize::MAX } else { 0 };
    let paths: Vec<String> = store
        .list_paths(&format!("{prefix}/"), ordered)
        .await?
        .into_iter()
        .filter(|path| path.ends_with(".json"))
        .collect();
    let mut jobs = Vec::new();
    for paths in paths.chunks(100) {
        let texts: Vec<Option<String>> = futures::stream::iter(paths)
            .map(|path| store.download_text(path))
            .buffered(10)
            .collect::<Vec<Result<Option<String>, StorageError>>>()
            .await
            .into_iter()
            .collect::<Result<Vec<_>, _>>()?;
        for data in texts.into_iter().flatten() {
            let job = Job::from_json(&data)?;
            if is_transition_sentinel_state(&job.state) {
                continue;
            }
            jobs.push(job);
            if oldest_first > 0 && jobs.len() >= oldest_first {
                return Ok(jobs);
            }
        }
    }
    Ok(jobs)
}

/// Python `_download_or_none` fanned out over `paths` with `workers`
/// concurrent fetches. `ThreadPoolExecutor(max_workers=...)` + `pool.map`
/// becomes `buffered(workers)`, which preserves the path order in the
/// output, as `pool.map` does.
async fn download_many_or_none(
    store: &JobStorage,
    paths: &[String],
    workers: usize,
) -> Vec<Option<String>> {
    futures::stream::iter(paths)
        .map(|path| async move { store.download_text(path).await.ok().flatten() })
        .buffered(workers.max(1))
        .collect()
        .await
}

/// What a caller can actually run, and how much scanning it will pay to find
/// it.
///
/// The window used to be counted in jobs that merely FIT the caller's VRAM,
/// while the caller then refused most of them on accelerator, platform,
/// architecture, provider, assignment, exclusivity and slot state. With a
/// centrally assigned queue that is a permanent starvation, not a hiccup: if
/// the first `want` fitting blobs all name another worker, this worker gets a
/// page of jobs it must refuse, refuses every one of them, and idles on every
/// poll while its own job sits one place past the window. So the caller's own
/// full admission predicate decides what consumes a window slot, and the
/// scanning cost is bounded separately by [`JobScan::scan_budget`] — the two
/// are different quantities and conflating them is what produced both faults.
pub struct JobScan<'a> {
    /// Jobs to return. 0 means "every eligible job in the prefix".
    pub want: usize,
    /// Job documents this scan may download while looking for them. 0 means
    /// "as many as the prefix holds". A scan that exhausts its budget returns
    /// what it found; the next poll starts from the same ordered head, so
    /// nothing is permanently unreachable.
    pub scan_budget: usize,
    /// Cheap pre-download filter off the blob's stamped `gpu_mem_gb`, so a job
    /// that cannot fit is never fetched. `i64::MAX` disables it.
    pub max_gpu_mem_gb: i64,
    /// The caller's full admission rule, applied before a job takes a window
    /// slot. It sees the listed generation of the document; a caller that
    /// re-reads the job before claiming still has to re-apply it.
    pub eligible: &'a (dyn Fn(&Job) -> bool + Sync),
}

impl JobScan<'_> {
    fn window_full(&self, found: usize) -> bool {
        self.want > 0 && found >= self.want
    }

    fn budget_spent(&self, scanned: usize) -> bool {
        self.scan_budget > 0 && scanned >= self.scan_budget
    }

    fn accepts(&self, job: &Job) -> bool {
        !is_transition_sentinel_state(&job.state)
            && job.gpu_mem_gb <= self.max_gpu_mem_gb
            && (self.eligible)(job)
    }
}

/// The ordered index first, deduped by job_id, counting only jobs the caller
/// can actually claim.
///
/// The index is the whole listing strategy for `queue/` now, not a fast path
/// in front of one. `queue_priority/<inv_priority>-<created_at>-<job_id>.json`
/// sorts, by name, into exactly the order the scheduler wants — priority
/// descending, then oldest first — so walking the name-ordered prefix a page
/// at a time and stopping when the window is full reads a handful of names
/// instead of materializing 14k of them and cutting afterwards. The cap used
/// to bound only what was RETURNED, which is why a cloud backend still paid
/// for the entire prefix on every poll.
///
/// Stale markers are expected and harmless. A job leaves `queue/` after its
/// marker was written, and older versions dropped markers only on the
/// queue -> running path. A marker whose job is gone, or whose job is no
/// longer claimable, costs scan budget and is skipped; it never consumes a
/// window slot, so dead markers cannot hide live priority jobs behind them.
///
/// The `queue/` blob stays the source of truth: the marker only says which
/// job to look at, and every decision is made on the job document itself.
/// Which is why an unindexed job must still be reachable — see the fallback
/// below.
pub async fn list_claimable(
    store: &JobStorage,
    prefix: &str,
    scan: &JobScan<'_>,
) -> Result<Vec<Job>, StorageError> {
    let mut out: Vec<Job> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    let mut scanned = 0usize;
    // The index names queued jobs and nothing else; resolving it against
    // another prefix would read whatever job happens to share the id.
    if prefix != "queue" {
        collect_oldest_first(store, prefix, scan, &mut out, &mut seen, &mut scanned).await?;
        return Ok(out);
    }
    // Extends the existing backfill rather than adding a second repair: it
    // now covers every queued job, because the index now does. `true` means
    // the index is known to name every job in `queue/`.
    let indexed = migrations::backfill_priority_markers(store, migrations::BACKFILL_BATCH).await?;
    collect_from_index(store, prefix, scan, &mut out, &mut seen, &mut scanned).await?;
    if !indexed {
        // The index does not name everything yet, so it cannot be the only
        // way in without stranding the jobs it has not reached. This pass is
        // the expensive one the index exists to retire, and it retires itself:
        // the backfill above advances on every call, and once it reports
        // complete this branch is dead for the life of the store.
        collect_oldest_first(store, prefix, scan, &mut out, &mut seen, &mut scanned).await?;
    }
    Ok(out)
}

/// The ordered `queue_priority/` walk: pages of names, resolved against
/// `queue/`, stopping on a full window or a spent budget.
///
/// Resumable. A bounded scan starts where the last one stopped and wraps at
/// the end of the prefix, so a job past the budget is reached on a later poll
/// instead of never: a budget anchored at a fixed head bounds the cost but
/// re-reads the same head forever, which is the starvation it was added to
/// prevent. An unbounded scan ignores the cursor and walks the whole index
/// from the head — it starves nothing, and it has no next poll to hand off to.
///
/// The cursor only moves when a scan was actually cut short. A scan that
/// reached the end of the index records an empty cursor and so starts again
/// at the head, which means an index that fits inside one poll's window and
/// budget is always read in strict priority order and the rotation never
/// engages. It engages exactly when the index is bigger than one poll can
/// hold — the case where something past the window would otherwise never be
/// looked at.
async fn collect_from_index(
    store: &JobStorage,
    prefix: &str,
    scan: &JobScan<'_>,
    out: &mut Vec<Job>,
    seen: &mut HashSet<String>,
    scanned: &mut usize,
) -> Result<(), StorageError> {
    if scan.window_full(out.len()) || scan.budget_spent(*scanned) {
        return Ok(());
    }
    let bounded = scan.want > 0 || scan.scan_budget > 0;
    let origin = if bounded { store.scan_cursor() } else { String::new() };
    let mut at = origin.clone();
    let mut wrapped = false;
    // Every exit records where it stopped, including the exits that found
    // nothing: a scan whose whole page was stale markers has to leave that
    // page behind or the next poll repeats it. An empty cursor means "the
    // index was walked to the end" and sends the next scan back to the head.
    let stopped_at: String;
    'walk: loop {
        let page = store.list_page(MARKER_PREFIX, &at, MARKER_PAGE).await?;
        let Some(page_end) = page.last().cloned() else {
            // End of the prefix. A bounded scan that started past the head
            // wraps once to cover what it skipped; anything else is done.
            if wrapped || origin.is_empty() {
                stopped_at = String::new();
                break 'walk;
            }
            at = String::new();
            wrapped = true;
            continue 'walk;
        };
        // Resolve this page: marker bodies first, then the jobs they name.
        // The body is the only place the job_id is stated unambiguously — see
        // [`is_marker`] for why the name cannot be parsed for it — so this is
        // two fan-outs per page. It still reads far less than the pass it
        // replaces, which listed the whole prefix and fetched every body in
        // it before anything could be cut.
        let markers: Vec<&String> = page.iter().filter(|path| is_marker(path)).collect();
        let marker_paths: Vec<String> = markers.iter().map(|path| (*path).clone()).collect();
        let marker_bodies =
            download_many_or_none(store, &marker_paths, 10.min(marker_paths.len())).await;
        let mut entries: Vec<(&str, String)> = Vec::new();
        for (marker, body) in markers.iter().zip(marker_bodies) {
            let Some(body) = body else {
                // The marker was deleted between the listing and this read:
                // its job left the queue, which is exactly the state the
                // reader is meant to shrug at.
                continue;
            };
            // Strict-raise on corrupt marker JSON; a missing/non-string
            // job_id just skips the marker.
            let value: serde_json::Value = serde_json::from_str(&body)?;
            let Some(job_id) = value.get("job_id").and_then(serde_json::Value::as_str) else {
                continue;
            };
            if !seen.contains(job_id) {
                entries.push((marker.as_str(), job_id.to_string()));
            }
        }
        let job_paths: Vec<String> = entries
            .iter()
            .map(|(_, job_id)| format!("{prefix}/{job_id}.json"))
            .collect();
        let bodies = download_many_or_none(store, &job_paths, 10.min(job_paths.len())).await;
        for ((marker, _), body) in entries.iter().zip(bodies) {
            // Once the wrapped leg reaches past the name the walk began at,
            // the whole index has been seen exactly once.
            if wrapped && !origin.is_empty() && *marker > origin.as_str() {
                stopped_at = String::new();
                break 'walk;
            }
            // A marker with no job behind it is the expected stale case: skip
            // it, charge the budget, keep going. It cost a read, so it costs
            // budget; it never costs a window slot. The budget check below
            // runs for it too — a page of nothing but dead markers still has
            // to be able to exhaust the budget, or the "bound" would be no
            // bound at all on exactly the input that needs one.
            *scanned += 1;
            if let Some(data) = body {
                let job = Job::from_json(&data)?;
                if scan.accepts(&job) && seen.insert(job.job_id.clone()) {
                    out.push(job);
                    if scan.window_full(out.len()) {
                        stopped_at = (*marker).to_string();
                        break 'walk;
                    }
                }
            }
            if scan.budget_spent(*scanned) {
                stopped_at = (*marker).to_string();
                break 'walk;
            }
        }
        // The walk must advance. `list_page` is contracted to return names
        // strictly after `at`, so `page_end` is always past it; a backend that
        // got that wrong would spin here forever, and a scheduler poll that
        // never returns is worse than one that returns short.
        if page_end <= at && !at.is_empty() {
            stopped_at = String::new();
            break 'walk;
        }
        at = page_end;
    }
    if bounded {
        store.set_scan_cursor(stopped_at);
    }
    Ok(())
}

/// The oldest-first pass over the prefix itself.
///
/// The prefix is listed once, with metadata, and ordered by write time before
/// anything is downloaded. Ordering after a cap is what starved a late-sorting
/// job; re-listing to widen a budget is what made a poll cost the whole prefix
/// repeatedly. One listing, one order, early exit.
async fn collect_oldest_first(
    store: &JobStorage,
    prefix: &str,
    scan: &JobScan<'_>,
    out: &mut Vec<Job>,
    seen: &mut HashSet<String>,
    scanned: &mut usize,
) -> Result<(), StorageError> {
    if scan.window_full(out.len()) || scan.budget_spent(*scanned) {
        return Ok(());
    }
    let mut ordered: Vec<(chrono::DateTime<chrono::Utc>, String)> = Vec::new();
    for blob in store.list_blobs_with_meta(&format!("{prefix}/")).await? {
        let name = blob.name.rsplit('/').next().unwrap_or("");
        let job_id = name.strip_suffix(".json").unwrap_or(name);
        if !blob.name.ends_with(".json") || seen.contains(job_id) {
            continue;
        }
        // A blob that predates the metadata stamp carries no gpu_mem_gb and
        // is downloaded rather than assumed unfit. A corrupt integer raises,
        // so misbehaving metadata is reported instead of silently filtering.
        if let Some(mem_str) = blob.metadata.get("gpu_mem_gb") {
            let mem: i64 = mem_str.parse().map_err(|_| {
                StorageError::Other(format!(
                    "corrupt gpu_mem_gb metadata on {}: {mem_str:?}",
                    blob.name
                ))
            })?;
            if mem > scan.max_gpu_mem_gb {
                continue;
            }
        }
        ordered.push((
            blob.updated.unwrap_or_else(chrono::Utc::now),
            blob.name.clone(),
        ));
    }
    ordered.sort();
    let paths: Vec<String> = ordered.into_iter().map(|(_, name)| name).collect();
    for paths in paths.chunks(32) {
        let texts: Vec<Option<String>> = futures::stream::iter(paths)
            .map(|path| store.download_text(path))
            .buffered(32)
            .collect::<Vec<Result<Option<String>, StorageError>>>()
            .await
            .into_iter()
            .collect::<Result<Vec<_>, _>>()?;
        for data in texts.into_iter().flatten() {
            let job = Job::from_json(&data)?;
            *scanned += 1;
            if scan.accepts(&job) && seen.insert(job.job_id.clone()) {
                out.push(job);
                if scan.window_full(out.len()) {
                    return Ok(());
                }
            }
            if scan.budget_spent(*scanned) {
                return Ok(());
            }
        }
    }
    Ok(())
}
