//! Phantom-job reaper: provider-neutral recovery of jobs whose worker died.
//!
//! Distinct from the two existing reapers:
//! - [`crate::monitor::monitor`] (`check_running_jobs` / `reap_dead_agents`)
//!   runs per CLOUD provider arm of the coordinator tick and deletes dead
//!   agent VMs. A fleet with no cloud arm (local/box workers only, or a
//!   provider API outage failing the arm) never runs it, so a worker that
//!   dies mid-job leaves the job in `running/` forever — phantom capacity
//!   the scheduler keeps counting, with no live worker behind it.
//! - [`crate::monitor::reap`] deletes per-job blobs of fully-terminal runs
//!   and never touches live records.
//!
//! This reaper keys on the job's worker lease, and the lease lives IN the
//! running job document (`Job::lease_expires_at`), renewed by
//! [`crate::queue::storage::JobStorage::renew_running_lease`] on the worker's
//! poll period from `write_heartbeat`. Each renewal states the worker's own
//! promise — its poll period plus its measured lateness and write time — and
//! the reaper holds the job to exactly that, deferring when the job wrote a
//! pulse or a checkpoint after it (see
//! [`crate::monitor::heartbeat_guard::job_liveness`]).
//!
//! Why in the document: while the lease was only the `status/<job_id>/heartbeat`
//! blob, no amount of re-reading could fence this reaper. It read the running
//! job at version V, read the pulse, and moved the job at V; a live worker
//! that refreshed its pulse in that window changed nothing the reaper held,
//! so the move succeeded, the job was requeued and a second worker started it
//! while the first was still executing and about to publish its result. A
//! renewal that is a compare-and-swap on the running document invalidates V,
//! so the reaper's version-pinned move fails and it loses the race instead of
//! silently winning it. A job claimed before promises existed carries none
//! and is kept and counted (`unpromised`): nothing says when it should have
//! spoken.
//!
//! Retry semantics: a stale release worker whose complete canonical receipt and
//! archive still verify against its immutable request is moved directly to
//! `completed/`; the evidence is the result, so rebuilding it would throw away
//! a successful qualification. Every other first lease expiry moves the job
//! back to `queue/` exactly once, incrementing the existing `restarts` retry
//! field (still bounded by `max_restarts`) and storing
//! [`LEASE_EXPIRED_REASON`] in `job.error` — both the diagnosis readers surface
//! and the marker that a second expiry turns the job `failed/` with that same
//! stored reason.
//!
//! Write discipline: every transition uses
//! [`JobStorage::move_job_if_version`], which persists explicit ownership and
//! source-generation intent, CAS-fences the source, then creates or validates
//! the destination. Any caller can finish an abandoned transition.
//!
//! The pieces sit beside this entry point: `expiry` is one running job's
//! transition, `release_output` the receipt-and-archive verification that
//! transition defers to, and `assignments` the silent-worker pass over
//! `queue/`.

mod assignments;
mod expiry;
mod release_output;

use chrono::Utc;

use super::{JobStorage, StorageError};
use assignments::clear_silent_assignments;
use expiry::reap_one;

/// Stored `job.error` reason for a lease-expiry transition. Written on the
/// first-expiry requeue (where it doubles as the "already requeued once"
/// marker) and kept as the terminal reason on the second-expiry failure.
pub const LEASE_EXPIRED_REASON: &str = "worker lease expired";

/// One tick's worth of reaper work.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReaperSummary {
    /// Running jobs moved back to `queue/` on their first lease expiry.
    pub requeued: usize,
    /// Running jobs moved to `failed/` on their second lease expiry (or
    /// with the restart budget already spent).
    pub failed: usize,
    /// Release jobs completed from a verified canonical receipt and archive
    /// after their worker lease expired.
    pub release_completions: usize,
    /// Queued jobs whose `assigned_to` named a silent worker, cleared so
    /// another worker can claim them.
    pub assignments_cleared: usize,
    /// Whether the marker-index sweep has covered `queue/` end-to-end at
    /// least once, which is what lets the listing walk drop its
    /// whole-prefix pass. Reported, not acted on, here.
    pub index_swept: bool,
    /// Cleaned transition sentinels of terminal jobs deleted from `queue/`
    /// and `running/` this pass, so the prefixes hold live work and not the
    /// history of every job that ever passed through.
    pub sentinels_retired: usize,
    /// Running jobs this pass could not read or decide, each logged with its
    /// error. Every other lease is still reaped: one record the reaper cannot
    /// derive must not leave the dead jobs behind it holding their slots.
    pub unreadable: usize,
    /// Running jobs whose record carries no worker promise (claimed by a
    /// Stado from before promises), kept because nothing says when their
    /// worker should have spoken.
    pub unpromised: usize,
}

/// Reap one job, and on failure log the job and its error and count it, so the
/// pass goes on to the next lease.
async fn reap_or_report(
    store: &JobStorage,
    job_id: &str,
    now: chrono::DateTime<Utc>,
    log: &dyn Fn(&str),
    summary: &mut ReaperSummary,
) {
    if let Err(error) = reap_one(store, job_id, now, log, summary).await {
        summary.unreadable += 1;
        log(&format!("{job_id}: not reaped: {error}"));
    }
}

/// One reaper pass over the queue: repair the marker index, recover phantom
/// running jobs, then release queued jobs pinned to silent workers. Called
/// from the coordinator tick before assignment so recovered work is
/// dispatchable in the same tick.
pub async fn reap_expired_leases(
    store: &JobStorage,
    log: &dyn Fn(&str),
) -> Result<ReaperSummary, StorageError> {
    let now = Utc::now();
    // The index repair belongs on this tick, and it never retires. A queued
    // job whose marker write did not land is invisible to every scheduler
    // while still reporting `queued` — the same class of stranding this
    // reaper exists to undo, just on the listing index instead of the lease.
    // The sweep covers every queued job on every tick, so a lost marker is
    // rewritten on the next one. It runs before the passes below so a
    // recovered marker is claimable in this same tick.
    let index_swept = crate::queue::migrations::backfill_priority_markers(store).await?;
    let mut summary = ReaperSummary {
        index_swept,
        ..Default::default()
    };
    for candidate in store.list_jobs("running", 0).await? {
        if candidate.job_id.is_empty() {
            continue;
        }
        reap_or_report(store, &candidate.job_id, now, log, &mut summary).await;
    }
    clear_silent_assignments(store, now, log, &mut summary).await?;
    // Last, because it is bookkeeping: every sentinel of a settled terminal
    // job is swept; a transition still finishing is not retired and stays.
    summary.sentinels_retired += retire_sentinels(store, "queue", log).await?;
    summary.sentinels_retired += retire_sentinels(store, "running", log).await?;
    Ok(summary)
}

/// Retire the settled sentinels under one prefix and log what was retired.
async fn retire_sentinels(
    store: &JobStorage,
    prefix: &str,
    log: &dyn Fn(&str),
) -> Result<usize, StorageError> {
    let sweep = store.retire_settled_sentinels(prefix).await?;
    if sweep.retired > 0 {
        log(&format!(
            "reaper: {prefix}/ settled sentinels retired={} kept={} inspected={}",
            sweep.retired, sweep.kept, sweep.inspected,
        ));
    }
    Ok(sweep.retired)
}

/// The same expiry decision for named running jobs only, without the
/// fleet-wide listing, index repair and sentinel sweep: what a worker runs
/// for the jobs it finished itself, whose running documents it knows by id.
/// A job that is not running, or whose lease is still live, is left alone,
/// exactly as in [`reap_expired_leases`].
pub async fn reap_named(
    store: &JobStorage,
    job_ids: &[String],
    log: &dyn Fn(&str),
) -> Result<ReaperSummary, StorageError> {
    let now = Utc::now();
    let mut summary = ReaperSummary::default();
    for job_id in job_ids.iter().filter(|job_id| !job_id.is_empty()) {
        reap_or_report(store, job_id, now, log, &mut summary).await;
    }
    Ok(summary)
}
