//! What a scheduler poll can actually claim: the entry point, and the two
//! passes it drives.
//!
//! The pieces sit beside it: `scan` holds the window, the budget and the
//! caller's admission rule, `index` is the ordered priority-marker walk with
//! its resumable cursor, and `oldest_first` is the whole-prefix pass the
//! index exists to retire.

mod index;
mod oldest_first;
mod scan;

use std::collections::HashSet;

use crate::models::Job;
use crate::queue::storage::JobStorage;
use crate::queue::{migrations, StorageError};

use index::collect_from_index;
use oldest_first::collect_oldest_first;

pub use scan::JobScan;

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
/// Which is why an unindexed job must still be reachable — see the
/// whole-prefix pass below.
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
    // Coverage is a one-object read; the sweep that establishes it is not.
    // Asking [`migrations::has_swept`] first is what keeps a poll down to a
    // page of names: driving the sweep from here unconditionally would make
    // every poll list all of `queue/` AND all of `queue_priority/` and fetch
    // up to a batch of job documents — the whole-prefix per-poll cost this
    // walk exists to remove, and the documented way to blow a 60s tick. The
    // standing bounded repair runs per coordinator tick in
    // `queue::reaper::reap_expired_leases`, which is where that cost belongs.
    let indexed = migrations::has_swept(store).await?;
    if !indexed {
        // Only while the whole-prefix pass is still live: the sweep runs
        // before the index is read so a marker it writes is claimable on this
        // poll, and it is what eventually retires that pass below.
        migrations::backfill_priority_markers(store, migrations::BACKFILL_BATCH).await?;
    }
    collect_from_index(store, prefix, scan, &mut out, &mut seen, &mut scanned).await?;
    if !indexed {
        // No sweep has covered the whole prefix yet, so the index cannot be
        // the only way in without stranding the jobs it has not reached. This
        // pass is the expensive one the index exists to retire, and what
        // retires it is coverage: the sweep just above advances on every poll
        // that lands here, and once one reaches the tail both this branch and
        // that sweep stop running from polls — the bounded repair keeps going
        // on the coordinator tick instead.
        collect_oldest_first(store, prefix, scan, &mut out, &mut seen, &mut scanned).await?;
    }
    Ok(out)
}
