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

/// Index entry for priority>0 jobs.
pub async fn write_marker(store: &JobStorage, job: &Job) -> Result<(), StorageError> {
    let name = format!("queue_priority/{}-{}.json", priority_key(job), job.job_id);
    // Python `json.dumps({"job_id": ..., "priority": int(...)})` with
    // default separators.
    let body = format!(
        "{{\"job_id\": {}, \"priority\": {}}}",
        json_str(&job.job_id),
        job.priority
    );
    store.upload_text(&name, &body).await
}

/// Remove any priority marker(s) for this job_id.
pub async fn delete_marker(store: &JobStorage, job_id: &str) -> Result<(), StorageError> {
    let suffix = format!("-{job_id}.json");
    for path in store.list_paths("queue_priority/", 0).await? {
        if path.ends_with(&suffix) {
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

/// Priority markers first, then oldest-first, deduped by job_id, counting only
/// jobs the caller can actually claim.
///
/// Stale priority markers are expected: a job can move out of queue/ after its
/// marker was written, and older versions only deleted markers on the
/// queue -> running path. A stale or ineligible marker costs scan budget, never
/// a window slot, so dead markers cannot hide the priority jobs behind them.
///
/// Calls [`migrations::backfill_priority_markers`] up-front so any pre-0.4.26
/// queued job gets a retroactive marker.
pub async fn list_claimable(
    store: &JobStorage,
    prefix: &str,
    scan: &JobScan<'_>,
) -> Result<Vec<Job>, StorageError> {
    migrations::backfill_priority_markers(store, migrations::BACKFILL_BATCH).await?;
    let mut out: Vec<Job> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    let mut scanned = 0usize;
    // The marker index names queued jobs and nothing else; resolving it
    // against another prefix would read whatever happens to share the id.
    if prefix == "queue" {
        collect_priority(store, prefix, scan, &mut out, &mut seen, &mut scanned).await?;
    }
    collect_oldest_first(store, prefix, scan, &mut out, &mut seen, &mut scanned).await?;
    Ok(out)
}

/// The `queue_priority/` index pass. Markers sort ascending by name =
/// (inv_priority, created_at), so walking them in order is priority-desc then
/// FIFO.
async fn collect_priority(
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
    let marker_paths: Vec<String> = store
        .list_paths("queue_priority/", 0)
        .await?
        .into_iter()
        .filter(|path| path.ends_with(".json"))
        .collect();
    for markers in marker_paths.chunks(50) {
        let bodies = download_many_or_none(store, markers, 10.min(markers.len())).await;
        let mut job_ids: Vec<String> = Vec::new();
        for body in bodies.iter().flatten() {
            // Strict-raise on corrupt marker JSON; a missing/non-string
            // job_id just skips the marker.
            let value: serde_json::Value = serde_json::from_str(body)?;
            if let Some(job_id) = value.get("job_id").and_then(serde_json::Value::as_str) {
                if !seen.contains(job_id) {
                    job_ids.push(job_id.to_string());
                }
            }
        }
        if job_ids.is_empty() {
            continue;
        }
        let job_paths: Vec<String> = job_ids
            .iter()
            .map(|job_id| format!("{prefix}/{job_id}.json"))
            .collect();
        let blobs = download_many_or_none(store, &job_paths, 10.min(job_paths.len())).await;
        for data in blobs.into_iter().flatten() {
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
