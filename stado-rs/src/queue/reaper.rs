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
//! This reaper keys on the job's worker lease: the
//! `status/<job_id>/heartbeat` blob every executor refreshes on
//! [`crate::providers::local::slots::HEARTBEAT_INTERVAL_S`]
//! (`write_heartbeat`), falling back to `started_at` for a worker that died
//! before its first heartbeat. The TTL is the codebase's own
//! [`crate::config::HEARTBEAT_STALE_MINUTES`] — the window
//! [`super::control::default_drain_timeout_s`] documents as "the window
//! after which the monitor declares a running job's heartbeat dead".
//!
//! Retry semantics: the first lease expiry moves the job back to `queue/`
//! exactly once, incrementing the existing `restarts` retry field (still
//! bounded by `max_restarts`) and storing [`LEASE_EXPIRED_REASON`] in
//! `job.error` — both the diagnosis readers surface and the marker that a
//! second expiry turns the job `failed/` with that same stored reason.
//!
//! Write discipline: every transition is fenced per job record — versioned
//! read, create-if-absent at the destination (the `claim_queued_job`
//! transition claim), then compare-and-swap on the source record against
//! the version read. A lost race compensates by deleting the destination
//! claim and defers to the winner; a fresh heartbeat is re-checked from
//! storage metadata immediately before the transition so a job whose
//! worker heartbeat is fresh is never touched.

use chrono::Utc;

use crate::config;
use crate::models::{isoformat_utc, job_state, Job};
use crate::monitor::heartbeat_guard as hg;

use super::{capacity, JobStorage, StorageError};

/// Stored `job.error` reason for a lease-expiry transition. Written on the
/// first-expiry requeue (where it doubles as the "already requeued once"
/// marker) and kept as the terminal reason on the second-expiry failure.
pub const LEASE_EXPIRED_REASON: &str = "worker lease expired";

/// Checkpoint-freshness defer window, the same 5400s the monitor's orphan
/// branch uses: a multi-GB checkpoint upload can starve the heartbeat PUT
/// while the job is demonstrably alive (2026-05-16/17 incidents).
const CHECKPOINT_FRESH_SECONDS: f64 = 5400.0;

/// One tick's worth of reaper work.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReaperSummary {
    /// Running jobs moved back to `queue/` on their first lease expiry.
    pub requeued: usize,
    /// Running jobs moved to `failed/` on their second lease expiry (or
    /// with the restart budget already spent).
    pub failed: usize,
    /// Queued jobs whose `assigned_to` named a silent worker, cleared so
    /// another worker can claim them.
    pub assignments_cleared: usize,
}

/// Seconds since `status/<job_id>/heartbeat` was last written, or `None`
/// when no heartbeat blob exists (a worker that died before its first
/// write, or a just-requeued job whose status blobs were cleaned).
async fn heartbeat_age_seconds(
    store: &JobStorage,
    job_id: &str,
    now: chrono::DateTime<Utc>,
) -> Result<Option<i64>, StorageError> {
    let path = format!("status/{job_id}/heartbeat");
    Ok(store
        .backend()
        .updated_at(&path)
        .await?
        .map(|updated| (now - updated).num_seconds()))
}

/// Seconds since the job was (last) started, per its `started_at` stamp.
fn started_age_seconds(job: &Job, now: chrono::DateTime<Utc>) -> Option<i64> {
    let started = hg::parse_iso_lenient(job.started_at.as_deref().filter(|s| !s.is_empty())?)?;
    Some((now - started).num_seconds())
}

/// Fenced prefix move of one job record.
///
/// The create-if-absent at the destination is the transition claim (the
/// `claim_queued_job` discipline): exactly one mover wins it. The
/// compare-and-swap on the source record against `expected_version` proves
/// no concurrent writer touched (or terminally moved) the record since the
/// caller's read; on a lost race the destination claim is deleted again so
/// the store is left exactly as found. Returns `true` when this caller
/// performed the move.
async fn fenced_move(
    store: &JobStorage,
    job: &Job,
    from_prefix: &str,
    to_prefix: &str,
    expected_version: &str,
) -> Result<bool, StorageError> {
    let from = format!("{from_prefix}/{}.json", job.job_id);
    let to = format!("{to_prefix}/{}.json", job.job_id);
    let body = job.to_json();
    if !store.create_text_if_absent(&to, &body).await? {
        return Ok(false);
    }
    match store.compare_and_swap_text(&from, expected_version, &body).await {
        Ok(_) => {
            store.delete_blob(&from).await?;
            if from_prefix == "queue" {
                store.delete_priority_marker(&job.job_id).await?;
            }
            super::tombstone::on_transition(store, job, to_prefix).await;
            Ok(true)
        }
        Err(StorageError::StorageConflict(_) | StorageError::NotFound(_)) => {
            store.delete_blob(&to).await?;
            Ok(false)
        }
        Err(error) => {
            store.delete_blob(&to).await?;
            Err(error)
        }
    }
}

/// Reap one running job whose lease is expired: requeue on the first
/// expiry, fail on the second. Leaves the job alone (fresh, guarded, or
/// the fence lost to a concurrent writer) without counting it.
async fn reap_one(
    store: &JobStorage,
    job_id: &str,
    lease_ttl_seconds: i64,
    now: chrono::DateTime<Utc>,
    log: &dyn Fn(&str),
    summary: &mut ReaperSummary,
) -> Result<(), StorageError> {
    let path = format!("running/{job_id}.json");
    let Some(versioned) = store.read_text_versioned(&path).await? else {
        return Ok(()); // already moved by a concurrent writer
    };
    let mut job = Job::from_json(&versioned.content)?;
    if job.state != job_state::RUNNING {
        return Ok(());
    }

    // Expired iff EVERY liveness signal is older than the lease TTL: the
    // heartbeat blob when one exists, and started_at (boot grace — a
    // freshly claimed job has not written its first heartbeat yet). An
    // undateable job is skipped rather than reaped on an invented fact.
    let heartbeat_age = heartbeat_age_seconds(store, job_id, now).await?;
    let started_age = started_age_seconds(&job, now);
    if heartbeat_age.is_none() && started_age.is_none() {
        return Ok(());
    }
    let fresh = |age: Option<i64>| age.is_some_and(|age| age <= lease_ttl_seconds);
    if fresh(heartbeat_age) || fresh(started_age) {
        return Ok(());
    }
    // The freshest (smallest) stale age, named in the log line.
    let age = [heartbeat_age, started_age]
        .into_iter()
        .flatten()
        .min()
        .unwrap_or_default();

    // A command that kills `wc agent` itself is done, not orphaned: the
    // agent's disappearance is the success condition.
    if hg::finalize_if_self_terminating(store, &mut job, log).await? {
        return Ok(());
    }
    // A fresh checkpoint write is proof of life immune to the heartbeat
    // starvation a multi-GB upload causes; defer to it.
    if hg::any_job_checkpoint_fresh(store, &job, CHECKPOINT_FRESH_SECONDS).await {
        return Ok(());
    }

    let second_expiry = job.error.as_deref() == Some(LEASE_EXPIRED_REASON);
    if second_expiry || job.restarts + 1 > job.max_restarts {
        job.state = job_state::FAILED.to_string();
        job.failed_at = Some(isoformat_utc(now));
        job.error = Some(LEASE_EXPIRED_REASON.to_string());
        if fenced_move(store, &job, "running", "failed", &versioned.version).await? {
            summary.failed += 1;
            let why = if second_expiry {
                "second lease expiry".to_string()
            } else {
                format!("restart cap {} exceeded", job.max_restarts)
            };
            log(&format!(
                "{job_id}: FAILED ({LEASE_EXPIRED_REASON}; lease silent for {age}s; {why})"
            ));
        }
        return Ok(());
    }

    job.restarts += 1;
    job.state = job_state::QUEUED.to_string();
    job.instance_ref = None;
    job.started_at = None;
    job.last_restart = Some(isoformat_utc(now));
    job.error = Some(LEASE_EXPIRED_REASON.to_string());
    // The worker that held the lease is dead; leaving its name in
    // assigned_to would pin the job to phantom capacity (job_eligible
    // rejects every other claimant). Empty assigned_to is the documented
    // any-eligible-agent semantic. An operator hard-pin (pinned_host) is
    // the exception: assigned_to mirrors it, and
    // repair_conflicting_pinned_assignments restores the mirror anyway.
    if job.pinned_host.is_empty() {
        job.assigned_to = String::new();
    }
    if fenced_move(store, &job, "running", "queue", &versioned.version).await? {
        summary.requeued += 1;
        store.refresh_job_metadata("queue", &job).await?;
        if job.priority > 0 {
            store.write_priority_marker(&job).await?;
        }
        store.cleanup_status(&job.job_id).await?;
        log(&format!(
            "{job_id}: requeued ({LEASE_EXPIRED_REASON}; lease silent for {age}s; restart {}/{})",
            job.restarts, job.max_restarts
        ));
    }
    Ok(())
}

/// Clear `assigned_to` on queued jobs whose named worker has gone silent,
/// so another worker can claim them. Silence is the codebase's own
/// liveness horizon: absence from [`capacity::read_consumer_capacity`],
/// which drops every publication older than
/// [`capacity::CAPACITY_STALE_SECONDS`]. Operator hard-pins
/// (`pinned_host`) are never touched — the same rule the makespan matcher
/// follows.
async fn clear_silent_assignments(
    store: &JobStorage,
    now: chrono::DateTime<Utc>,
    log: &dyn Fn(&str),
    summary: &mut ReaperSummary,
) -> Result<(), StorageError> {
    let live = capacity::read_consumer_capacity(store).await?;
    for candidate in store.list_jobs("queue", 0).await? {
        if candidate.assigned_to.is_empty() || !candidate.pinned_host.is_empty() {
            continue;
        }
        let worker_live = live
            .keys()
            .any(|consumer| consumer.eq_ignore_ascii_case(&candidate.assigned_to));
        if worker_live {
            continue;
        }
        let job_id = candidate.job_id.clone();
        let path = format!("queue/{job_id}.json");
        let Some(versioned) = store.read_text_versioned(&path).await? else {
            continue;
        };
        let mut job = Job::from_json(&versioned.content)?;
        if job.state != job_state::QUEUED || job.assigned_to != candidate.assigned_to {
            continue; // changed under the listing; the next tick reconciles
        }
        let worker = std::mem::take(&mut job.assigned_to);
        match store
            .compare_and_swap_text(&path, &versioned.version, &job.to_json())
            .await
        {
            Ok(_) => {
                summary.assignments_cleared += 1;
                let broadcast = store
                    .backend()
                    .updated_at(&format!("{}{worker}.json", capacity::CAPACITY_PREFIX))
                    .await?;
                let silence = match broadcast {
                    Some(updated) => {
                        format!("last broadcast {}s ago", (now - updated).num_seconds())
                    }
                    None => "no capacity broadcast on record".to_string(),
                };
                log(&format!(
                    "{job_id}: cleared assignment to silent worker {worker} ({silence})"
                ));
            }
            Err(StorageError::StorageConflict(_)) => continue,
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

/// One reaper pass over the queue: recover phantom running jobs, then
/// release queued jobs pinned to silent workers. Called from the
/// coordinator tick before assignment so recovered work is dispatchable in
/// the same tick.
pub async fn reap_expired_leases(
    store: &JobStorage,
    log: &dyn Fn(&str),
) -> Result<ReaperSummary, StorageError> {
    let now = Utc::now();
    let lease_ttl_seconds = config::HEARTBEAT_STALE_MINUTES * 60;
    let mut summary = ReaperSummary::default();
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
