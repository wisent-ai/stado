//! The maintenance command whose own success kills the agent:
//! recognizing it, and finalizing such a job instead of requeuing it.

use chrono::Utc;

use crate::models::{isoformat_utc, job_state, Job};
use crate::queue::{JobStorage, StorageError};

/// True if the job command kills the `wc agent` process itself
/// (e.g. an upgrade-then-restart maintenance job:
/// `pip install --upgrade ... ; pkill -f "wc agent"`).
///
/// For such a command the agent's disappearance is the SUCCESS
/// condition, not an orphan failure. The agent dies before it can
/// write a COMPLETED status, so the job is stranded in running/ and
/// the orphan-reaper requeues it — which re-runs the kill on the
/// next agent generation, an infinite crash loop. Confirmed live
/// 2026-05-15: job 435b184e crash-looped ubuntu-server
/// (wisent-agent.service n_restarts=7) until removed by operator.
pub fn is_self_terminating_command(cmd: &str) -> bool {
    if cmd.is_empty() {
        return false;
    }
    (cmd.contains("pkill") || cmd.contains("kill ")) && cmd.contains("wc agent")
}

/// If job.command self-terminates the agent, finalize the job as
/// COMPLETED (running/ -> completed/) and return True. Otherwise
/// return False so the caller proceeds with its normal requeue path.
pub async fn finalize_if_self_terminating(
    store: &JobStorage,
    job: &mut Job,
    log_fn: &dyn Fn(&str),
) -> Result<bool, StorageError> {
    if !is_self_terminating_command(&job.command) {
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
