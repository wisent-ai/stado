//! One running job's expiry decision: which signal is the authority, the
//! deferrals that protect a worker that is demonstrably alive, and the
//! version-pinned move that follows.

use chrono::Utc;

use crate::models::{isoformat_utc, job_state, Job};
use crate::monitor::heartbeat_guard as hg;
use crate::queue::{JobStorage, StorageError};

use super::release_output::verified_release_completion;
use super::signals::{heartbeat_age_seconds, started_age_seconds};
use super::{ReaperSummary, LEASE_EXPIRED_REASON};

/// Checkpoint-freshness defer window, the same 5400s the monitor's orphan
/// branch uses: a multi-GB checkpoint upload can starve the heartbeat PUT
/// while the job is demonstrably alive (2026-05-16/17 incidents).
const CHECKPOINT_FRESH_SECONDS: f64 = 5400.0;

/// Reap one running job whose lease is expired: requeue on the first
/// expiry, fail on the second. Leaves the job alone (fresh, guarded, or
/// the fence lost to a concurrent writer) without counting it.
pub(super) async fn reap_one(
    store: &JobStorage,
    job_id: &str,
    lease_ttl_seconds: i64,
    now: chrono::DateTime<Utc>,
    log: &dyn Fn(&str),
    summary: &mut ReaperSummary,
) -> Result<(), StorageError> {
    store.recover_job_transition(job_id).await?;
    let path = format!("running/{job_id}.json");
    let Some(versioned) = store.read_text_versioned(&path).await? else {
        return Ok(()); // already moved by a concurrent writer
    };
    let mut job = Job::from_json(&versioned.content)?;
    if job.state != job_state::RUNNING {
        store.recover_job_transition(job_id).await?;
        return Ok(());
    }
    // The lease the worker renews INSIDE this document is the authority when
    // the document carries one. It is also the fence: the renewal is a
    // compare-and-swap on this very object, so a pulse that lands between
    // this read and the move below changes `versioned.version` and the
    // version-pinned move fails. That is the only construction that closes
    // the race — re-reading a heartbeat blob written beside the job cannot,
    // because nothing the reaper pins changes when it is written.
    //
    // A job claimed before the lease existed carries none. For those the old
    // signals still decide: the heartbeat blob when one exists, and
    // `started_at` as boot grace. An undateable job is skipped rather than
    // reaped on an invented fact.
    let lease_expiry = job
        .lease_expires_at
        .as_deref()
        .filter(|value| !value.is_empty())
        .and_then(hg::parse_iso_lenient);
    let heartbeat_age = heartbeat_age_seconds(store, job_id, now).await?;
    let started_age = started_age_seconds(&job, now);
    let fresh = |age: Option<i64>| age.is_some_and(|age| age <= lease_ttl_seconds);
    let age = match lease_expiry {
        Some(expires) => {
            if expires > now {
                return Ok(());
            }
            // An expired lease beside a FRESH pulse means the renewal write
            // is failing, not that the worker died: both come from the same
            // `write_heartbeat`, so a live executor whose compare-and-swap
            // keeps losing must not be reaped for the storage layer's fault.
            if fresh(heartbeat_age) {
                return Ok(());
            }
            (now - expires).num_seconds() + lease_ttl_seconds
        }
        None => {
            if heartbeat_age.is_none() && started_age.is_none() {
                return Ok(());
            }
            if fresh(heartbeat_age) || fresh(started_age) {
                return Ok(());
            }
            // The freshest (smallest) stale age, named in the log line.
            [heartbeat_age, started_age]
                .into_iter()
                .flatten()
                .min()
                .unwrap_or_default()
        }
    };

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

    // A job with no in-document lease has no fence at all, so every external
    // signal it does have is re-read immediately before the move: a worker
    // that refreshed its heartbeat or checkpoint while the finalizer and
    // first checkpoint inspection above were running must not be reaped from
    // the stale observation. A lease-bearing job needs none of this — its
    // renewal invalidates the version the move below is pinned to.
    if lease_expiry.is_none()
        && (fresh(heartbeat_age_seconds(store, job_id, now).await?)
            || hg::any_job_checkpoint_fresh(store, &job, CHECKPOINT_FRESH_SECONDS).await)
    {
        return Ok(());
    }

    // The worker can finish and durably publish its complete result just before
    // the owning agent is replaced. In that state retrying the build is both
    // wasteful and wrong: the immutable qualification already exists. This
    // runs only after the same stale-lease checks that protect every live job,
    // and the version-pinned transition below still loses to any late renewal.
    if let Some(completed_at) = verified_release_completion(store, &job, now, log).await? {
        job.state = job_state::COMPLETED.to_string();
        job.completed_at = Some(completed_at);
        job.failed_at = None;
        job.error = None;
        if store
            .move_job_if_version(&job, "running", "completed", &versioned.version)
            .await?
        {
            summary.release_completions += 1;
            log(&format!(
                "{job_id}: completed from verified durable release output after worker lease expiry"
            ));
        }
        return Ok(());
    }
    let second_expiry = job.error.as_deref() == Some(LEASE_EXPIRED_REASON);
    if second_expiry || job.restarts + 1 > job.max_restarts {
        job.state = job_state::FAILED.to_string();
        job.failed_at = Some(isoformat_utc(now));
        job.error = Some(LEASE_EXPIRED_REASON.to_string());
        if store
            .move_job_if_version(&job, "running", "failed", &versioned.version)
            .await?
        {
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
    if store
        .move_job_if_version(&job, "running", "queue", &versioned.version)
        .await?
    {
        summary.requeued += 1;
        store.cleanup_status(&job.job_id).await?;
        log(&format!(
            "{job_id}: requeued ({LEASE_EXPIRED_REASON}; lease silent for {age}s; restart {}/{})",
            job.restarts, job.max_restarts
        ));
    }
    Ok(())
}
