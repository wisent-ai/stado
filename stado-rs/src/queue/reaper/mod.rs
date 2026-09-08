//! Phantom-job reaper: provider-neutral recovery of jobs whose worker died.
//!
//! Distinct from the two existing reapers:
//! - [`crate::monitor::monitor`] (`check_running_jobs` / `reap_dead_agents`)
//!   runs per CLOUD provider arm of the coordinator tick and deletes dead
//!   agent VMs. A fleet with no cloud arm (local/box workers only, or a
//!   provider API outage failing the arm) never runs it, so a worker that
//!   dies mid-job leaves the job in `running/` forever — phantom capacity
//!   the scheduler keeps counting. Confirmed live 2026-08-19/20: two jobs
//!   sat in `running` with no live worker behind them for hours.
//! - [`crate::monitor::reap`] deletes per-job blobs of fully-terminal runs
//!   and never touches live records.
//!
//! This reaper keys on the job's worker lease, and the lease lives IN the
//! running job document (`Job::lease_expires_at`), renewed by
//! [`crate::queue::storage::JobStorage::renew_running_lease`] every
//! [`crate::providers::local::slots::HEARTBEAT_INTERVAL_S`] from
//! `write_heartbeat`. The TTL is the codebase's own
//! [`crate::config::HEARTBEAT_STALE_MINUTES`] — the window
//! [`super::control::default_drain_timeout_s`] documents as "the window
//! after which the monitor declares a running job's heartbeat dead".
//!
//! Why in the document: while the lease was only the `status/<job_id>/heartbeat`
//! blob, no amount of re-reading could fence this reaper. It read the running
//! job at version V, read the pulse, and moved the job at V; a live worker
//! that refreshed its pulse in that window changed nothing the reaper held,
//! so the move succeeded, the job was requeued and a second worker started it
//! while the first was still executing and about to publish its result. A
//! renewal that is a compare-and-swap on the running document invalidates V,
//! so the reaper's version-pinned move fails and it loses the race instead of
//! silently winning it. Jobs claimed before the lease existed carry none, and
//! for exactly those the heartbeat blob and `started_at` still decide, with a
//! re-read immediately before the move.
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
//! transition defers to, `signals` the heartbeat and `started_at` ages both
//! of them read, and `assignments` the silent-worker pass over `queue/`.

mod assignments;
mod expiry;
mod release_output;
mod signals;

use chrono::Utc;

use crate::config;

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
    let lease_ttl_seconds = config::HEARTBEAT_STALE_MINUTES * 60;
    // The index repair belongs on this tick, and it never retires. A queued
    // job whose marker write did not land is invisible to every scheduler
    // while still reporting `queued` — the same class of stranding this
    // reaper exists to undo, just on the listing index instead of the lease.
    // The sweep is bounded per call and its cursor wraps, so this is a fixed
    // cost per tick that eventually re-examines every queued job rather than
    // a one-shot migration that stops looking. It runs before the passes
    // below so a recovered marker is claimable in this same tick.
    let index_swept =
        crate::queue::migrations::backfill_priority_markers(store, config::MARKER_REPAIR_PER_TICK)
            .await?;
    let mut summary = ReaperSummary {
        index_swept,
        ..Default::default()
    };
    for candidate in store.list_jobs("running", 0).await? {
        if candidate.job_id.is_empty() {
            continue;
        }
        reap_one(
            store,
            &candidate.job_id,
            lease_ttl_seconds,
            now,
            log,
            &mut summary,
        )
        .await?;
    }
    clear_silent_assignments(store, now, log, &mut summary).await?;
    Ok(summary)
}
