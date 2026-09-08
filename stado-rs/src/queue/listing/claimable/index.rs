//! The ordered walk over the priority index: one page of names at a time,
//! the fan-out that resolves them, and the resumable cursor.

use std::collections::HashSet;

use futures::StreamExt;

use crate::models::Job;
use crate::queue::listing::{is_marker, MARKER_PREFIX};
use crate::queue::storage::JobStorage;
use crate::queue::StorageError;

use super::scan::JobScan;

/// How many marker names one page of the ordered walk pulls.
///
/// Large enough that a full window is normally one listing round trip, small
/// enough that a scan which stops early has not paid for a page it will
/// never look at.
const MARKER_PAGE: usize = 256;

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
///
/// The cursor is shared by every bounded scan against `queue/`, which is
/// what makes the rotation a fleet-wide guarantee of reachability rather
/// than a per-caller one — and also why a caller whose decision depends on
/// the index's actual head must say so with [`JobScan::from_head`] instead
/// of inheriting whatever slice the last poll left behind.
pub(super) async fn collect_from_index(
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
    // `from_head` opts out of the rotation in both directions: the walk
    // starts at the head and the cursor is not moved, so a priority-fidelity
    // read neither observes nor perturbs the claim loops sharing it.
    let rotates = (scan.want > 0 || scan.scan_budget > 0) && !scan.from_head;
    let origin = if rotates {
        store.scan_cursor()
    } else {
        String::new()
    };
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
            // A marker body that vanished (its job left the queue between the
            // listing and this read), one whose job_id is missing, and one
            // naming a job already in `seen` all end the same way: no window
            // slot. They are still charged, because each one cost a fetch.
            // Leaving them free is what would let a page of them scan without
            // bound — the very input the budget exists for — and the charge is
            // what makes the claim below true for every dead marker, not just
            // the ones whose job blob is gone.
            let resolved = match &body {
                // Strict-raise on corrupt marker JSON; a missing/non-string
                // job_id just skips the marker.
                Some(body) => serde_json::from_str::<serde_json::Value>(body)?
                    .get("job_id")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string),
                None => None,
            };
            match resolved {
                Some(job_id) if !seen.contains(&job_id) => {
                    entries.push((marker.as_str(), job_id));
                }
                _ => {
                    *scanned += 1;
                    if scan.budget_spent(*scanned) {
                        stopped_at = (*marker).to_string();
                        break 'walk;
                    }
                }
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
            // Every marker the walk reads costs exactly one unit of budget,
            // here or in the resolve loop above, whether or not a job comes
            // back. A marker with no job behind it is the expected stale
            // case: skip it, charge it, keep going — it never costs a window
            // slot. That uniformity is what lets a page of nothing but dead
            // markers exhaust the budget, which is the input the bound exists
            // for; charging only the ones that resolve would leave it no
            // bound at all there.
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
    if rotates {
        store.set_scan_cursor(stopped_at);
    }
    Ok(())
}
