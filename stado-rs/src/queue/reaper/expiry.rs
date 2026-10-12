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
    let job = Job::from_json(&versioned.content)?;
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
    settle(
        store,
        job,
        &versioned.version,
        now,
        LEASE_EXPIRED_REASON,
        &format!("{age}s past its worker's promise"),
        log,
        summary,
    )
    .await
}

/// Stored `job.error` reason when a running job is released by the worker
/// that replaced the one running it on the same host.
pub const WORKER_REPLACED_REASON: &str = "its worker was replaced while it ran";

/// Release every running job whose record names `instance` — this host's
/// worker — when that worker has just started and runs nothing: the worker
/// it replaced ran them (a host holds one worker through its role lock).
/// `still_running` answers, for a job id, the processes still running that
/// job on this host; a workload outlives its worker when it runs in its own
/// process group, and such a job is left to its lease and said so. Every
/// other one is settled exactly as a lapsed lease is, now instead of when the
/// dead worker's promise passes: requeued once, failed on the same loss
/// again, completed when its release output already verifies. Answers the
/// lines it would log.
pub async fn release_left_behind(
    store: &JobStorage,
    instance: &str,
    still_running: &dyn Fn(&str) -> Result<Vec<String>, String>,
) -> Result<Vec<String>, StorageError> {
    let lines = std::cell::RefCell::new(Vec::new());
    let log = |line: &str| lines.borrow_mut().push(line.to_string());
    let mut summary = ReaperSummary::default();
    let now = Utc::now();
    for job_id in store.list_job_ids("running").await? {
        store.recover_job_transition(&job_id).await?;
        let path = format!("running/{job_id}.json");
        let Some(versioned) = store.read_text_versioned(&path).await? else {
            continue;
        };
        let job = Job::from_json(&versioned.content)?;
        if job.state != job_state::RUNNING || job.instance_ref.as_deref() != Some(instance) {
            continue;
        }
        match still_running(&job_id) {
            Ok(processes) if processes.is_empty() => {}
            Ok(processes) => {
                log(&format!(
                    "{job_id}: left by the worker this one replaced, but still run here by {}; \
                     it keeps its lease",
                    processes.join(", ")
                ));
                continue;
            }
            Err(error) => {
                log(&format!(
                    "{job_id}: left by the worker this one replaced; whether a process still \
                     runs it could not be read ({error}), so it keeps its lease"
                ));
                continue;
            }
        }
        let why = match job.started_at.as_deref() {
            Some(started) => {
                format!("started {started} by the worker this one replaced on {instance}")
            }
            None => format!("claimed by the worker this one replaced on {instance}"),
        };
        settle(
            store,
            job,
            &versioned.version,
            now,
            WORKER_REPLACED_REASON,
            &why,
            &log,
            &mut summary,
        )
        .await?;
    }
    Ok(lines.into_inner())
}

/// Settle a running job whose worker is gone: completed from verified
/// release output, failed on the same loss repeated, else requeued once.
/// `reason` is what the record stores, `why` what the log line says.
// Each argument is one fact of the settlement a caller already holds; a
// struct would only carry the same eight names one level out.
#[allow(clippy::too_many_arguments)]
async fn settle(
    store: &JobStorage,
    mut job: Job,
    version: &str,
    now: chrono::DateTime<Utc>,
    reason: &str,
    why: &str,
    log: &dyn Fn(&str),
    summary: &mut ReaperSummary,
) -> Result<(), StorageError> {
    let job_id = job.job_id.clone();

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
            .move_job_if_version(&job, "running", "completed", version)
            .await?
        {
            summary.release_completions += 1;
            log(&format!(
                "{job_id}: completed from verified durable release output after its worker ended ({why})"
            ));
        }
        return Ok(());
    }
    if let Some(refusal) = job.restart_refusal(reason) {
        job.state = job_state::FAILED.to_string();
        job.failed_at = Some(isoformat_utc(now));
        job.error = Some(reason.to_string());
        if store
            .move_job_if_version(&job, "running", "failed", version)
            .await?
        {
            summary.failed += 1;
            log(&format!("{job_id}: FAILED ({why}; {refusal})"));
        }
        return Ok(());
    }

    job.restarts += 1;
    job.state = job_state::QUEUED.to_string();
    job.instance_ref = None;
    job.started_at = None;
    job.last_restart = Some(isoformat_utc(now));
    job.error = Some(reason.to_string());
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
        .move_job_if_version(&job, "running", "queue", version)
        .await?
    {
        summary.requeued += 1;
        store.cleanup_status(&job.job_id).await?;
        log(&format!(
            "{job_id}: requeued ({reason}; {why}; restart {})",
            job.restarts
        ));
    }
    Ok(())
}
