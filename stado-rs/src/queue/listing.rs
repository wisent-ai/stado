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

/// The job_id a marker name encodes, or `None` for the migration sentinel and
/// anything else that is not a marker.
///
/// The name carries the job_id, so the reader does not download the marker
/// body to learn which job to resolve: the body would be a second fetch per
/// candidate to recover what the name already said. `created_at` contributes
/// `-` characters of its own, so the id is the segment after the LAST `-`,
/// which is the same parse the backfill has always used.
pub fn marker_job_id(path: &str) -> Option<&str> {
    let name = path.rsplit('/').next()?;
    if name.starts_with('.') {
        return None;
    }
    let id = name.strip_suffix(".json")?.rsplit('-').next()?;
    (!id.is_empty()).then_some(id)
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
/// written under, by walking the index.
///
/// This is the only remaining reason to traverse, and it exists for the two
/// cases where the name is genuinely not computable: a marker orphaned from a
/// job that no longer exists, and a marker left under a superseded key after
/// a priority change (the pre-change key cannot be derived from the
/// post-change job). Everything on a hot path uses
/// [`delete_marker_for`] instead.
pub async fn delete_markers_scanning(
    store: &JobStorage,
    job_id: &str,
) -> Result<(), StorageError> {
    for path in store.list_paths(MARKER_PREFIX, 0).await? {
        if marker_job_id(&path) == Some(job_id) {
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
        // Marker name beside the job it names, for the real markers on this
        // page. The marker BODY is never fetched: its name already carried
        // the job_id, so reading it would be a second request per candidate
        // to recover something already in hand.
        let entries: Vec<(&str, &str)> = page
            .iter()
            .filter_map(|path| marker_job_id(path).map(|job_id| (path.as_str(), job_id)))
            .filter(|(_, job_id)| !seen.contains(*job_id))
            .collect();
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
