//! The maintenance job whose own success stops the agent: finalizing such a
//! job instead of requeuing it.

use chrono::Utc;

use crate::models::{isoformat_utc, job_state, Job};
use crate::queue::{JobStorage, StorageError};

/// If the job declares that it stops the agent running it
/// (`Job::terminates_agent`, set by `stado submit --terminates-agent`),
/// finalize it as COMPLETED (running/ -> completed/) and return true;
/// otherwise return false so the caller proceeds with its normal requeue.
///
/// For such a job the agent's disappearance is the success condition, not an
/// orphan failure: the agent dies before it can write a COMPLETED status, so
/// the job is stranded in running/, and requeuing it re-runs the stop on the
/// next agent generation, a crash loop. The submitter declares it; the job's
/// command text is not read, so a command that merely mentions killing a
/// process is never taken for one.
pub async fn finalize_if_self_terminating(
    store: &JobStorage,
    job: &mut Job,
    log_fn: &dyn Fn(&str),
) -> Result<bool, StorageError> {
    if !job.terminates_agent {
        return Ok(false);
    }
    job.state = job_state::COMPLETED.to_string();
    job.completed_at = Some(isoformat_utc(Utc::now()));
    job.instance_ref = None;
    store.move_job(job, "running", "completed").await?;
    store.cleanup_status(&job.job_id).await?;
    log_fn(&format!(
        "{}: COMPLETED (self-terminating maintenance cmd; \
         agent kill is the success condition, not an orphan)",
        job.job_id
    ));
    Ok(true)
}
