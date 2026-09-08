//! The Spot move: back to the queue against a preempt counter rather than
//! the restart budget, so a Spot-heavy job cannot spend its restarts on
//! preemptions alone.

use chrono::Utc;

use crate::models::{isoformat_utc, job_state, Job};
use crate::queue::JobStorage;

use super::super::{log, MonitorError};
use super::current::{current_running, running_lease_live};

/// Move a Spot job back to queue, counting preemption separately from
/// restarts. A merely missing fleet-list entry still respects a live worker
/// lease; a provider-confirmed TERMINATED instance does not, because it cannot
/// renew and its lifecycle is already authoritative.
pub(in crate::monitor::monitor) async fn requeue_preempted(
    store: &JobStorage,
    job: &mut Job,
    reason: &str,
    respect_live_lease: bool,
) -> Result<bool, MonitorError> {
    let Some((current, version)) = current_running(store, &job.job_id).await? else {
        return Ok(false);
    };
    if respect_live_lease && running_lease_live(&current) {
        log(&format!(
            "{}: live in-document lease; not requeued ({reason})",
            current.job_id
        ));
        *job = current;
        return Ok(false);
    }
    let mut next = current;
    next.preempt_count += 1;
    next.state = job_state::QUEUED.to_string();
    next.instance_ref = None;
    next.started_at = None;
    next.last_restart = Some(isoformat_utc(Utc::now()));
    let moved = store
        .move_job_if_version(&next, "running", "queue", &version)
        .await?;
    if moved {
        store.cleanup_status(&next.job_id).await?;
        log(&format!(
            "{}: requeued ({reason}, preempts={})",
            next.job_id, next.preempt_count
        ));
    } else {
        log(&format!(
            "{}: still holds its lease; not requeued ({reason})",
            next.job_id
        ));
    }
    *job = next;
    Ok(moved)
}
