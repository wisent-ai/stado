//! The other half of the repair: bounded deletion of index entries that
//! name no queued job.

use std::collections::HashSet;

use crate::queue::storage::JobStorage;
use crate::queue::{listing, StorageError};

use super::budgets::MARKER_PRUNE_PER_CALL;
use super::sentinel::SENTINEL_PATH;

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
///
/// [`super::listing::list_claimable`]: crate::queue::listing::list_claimable
/// [`super::listing::is_marker`]: crate::queue::listing::is_marker
/// [`super::listing::delete_markers_scanning`]: crate::queue::listing::delete_markers_scanning
pub(super) async fn prune_stale_markers(
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
                && listing::is_marker(path)
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
