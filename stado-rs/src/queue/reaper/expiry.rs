//! One running job's expiry decision: the promise its worker wrote is the
//! authority, what the job wrote after that promise defers it, and the
//! version-pinned move follows.

use chrono::Utc;

use crate::models::{isoformat_utc, job_state, Job};
use crate::monitor::heartbeat_guard::{self as hg, JobLiveness};
use crate::queue::{JobStorage, StorageError};

use super::release_output::verified_release_completion;
use super::{ReaperSummary, LEASE_EXPIRED_REASON};

/// Reap one running job whose worker's promise has passed with nothing
/// written since: requeue on the first expiry, fail on the second. Leaves
/// the job alone (promised, written after its promise, unpromised, or the
/// fence lost to a concurrent writer) without counting it.
pub(super) async fn reap_one(
    store: &JobStorage,
    job_id: &str,
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
    // The lease the worker renews INSIDE this document is the authority and
    // the fence: the renewal is a compare-and-swap on this very object, so a
    // renewal that lands between this read and the move below changes
    // `versioned.version` and the version-pinned move fails. A pulse or a
    // checkpoint written after the promise means the worker is alive and its
    // renewal is what is failing. A job claimed before promises existed
    // carries none and is kept: nothing says when it should have spoken.
    let expires = match hg::job_liveness(store, &job, now).await {
        JobLiveness::Lapsed(expires) => expires,
        JobLiveness::Unpromised => {
            summary.unpromised += 1;
            return Ok(());
        }
        JobLiveness::Promised(_) | JobLiveness::WrittenAfter(_) => return Ok(()),
    };
    let age = (now - expires).num_seconds();

    // A command that kills `wc agent` itself is done, not orphaned: the
    // agent's disappearance is the success condition.
    if hg::finalize_if_self_terminating(store, &mut job, log).await? {
        return Ok(());
    }

    // The worker can finish and durably publish its complete result just before
    // the owning agent is replaced. In that state retrying the build is both
    // wasteful and wrong: the immutable qualification already exists. This
    // runs only after the same stale-lease checks that protect every live job,
    // and the version-pinned transition below still loses to any late renewal.
    let started = job
        .started_at
        .as_deref()
        .filter(|value| !value.is_empty())
        .and_then(hg::parse_iso_lenient);
    let verified = match started {
        Some(started) => verified_release_completion(store, &job, started, now, log).await?,
        None => None,
    };
    if let Some(completed_at) = verified {
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
    if let Some(refusal) = job.restart_refusal(LEASE_EXPIRED_REASON) {
        job.state = job_state::FAILED.to_string();
        job.failed_at = Some(isoformat_utc(now));
        job.error = Some(LEASE_EXPIRED_REASON.to_string());
        if store
            .move_job_if_version(&job, "running", "failed", &versioned.version)
            .await?
        {
            summary.failed += 1;
            log(&format!(
                "{job_id}: FAILED ({age}s past its worker's promise; {refusal})"
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
            "{job_id}: requeued ({LEASE_EXPIRED_REASON}; {age}s past its worker's promise; restart {})",
            job.restarts
        ));
    }
    Ok(())
}
