//! The restart-counted move: back to the queue while the restart budget
//! lasts, to failed once it is spent, and the post-reap batch that walks a
//! reaped VM's jids through it.

use std::collections::BTreeMap;

use chrono::Utc;

use crate::models::{isoformat_utc, job_state, Job};
use crate::queue::JobStorage;

use super::super::{log, MonitorError};
use super::current::{current_running, running_lease_live};

/// Move job back to queue or fail if max restarts exceeded.
///
/// Returns true only when this call won the version fence and changed
/// lifecycle state. Callers that must kill the old worker do so only after
/// this returns true: killing first would destroy a worker whose concurrent
/// lease renewal correctly made the lifecycle move lose.
pub(in crate::monitor::monitor) async fn requeue(
    store: &JobStorage,
    job: &mut Job,
    reason: &str,
) -> Result<bool, MonitorError> {
    let Some((current, version)) = current_running(store, &job.job_id).await? else {
        return Ok(false); // moved or finished under the tick's listing
    };
    if running_lease_live(&current) {
        log(&format!(
            "{}: live in-document lease; not requeued ({reason})",
            current.job_id
        ));
        *job = current;
        return Ok(false);
    }
    let mut next = current;
    next.restarts += 1;
    if next.restarts > next.max_restarts {
        next.state = job_state::FAILED.to_string();
        next.failed_at = Some(isoformat_utc(Utc::now()));
        next.error = Some(format!(
            "Exceeded {} restarts ({reason})",
            next.max_restarts
        ));
        // Python parity: NO cleanup_status on the restart-cap path.
        let moved = store
            .move_job_if_version(&next, "running", "failed", &version)
            .await?;
        if moved {
            log(&format!("{}: FAILED (restart cap, {reason})", next.job_id));
        } else {
            log(&format!(
                "{}: still holds its lease; not failed ({reason})",
                next.job_id
            ));
        }
        *job = next;
        return Ok(moved);
    }

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
            "{}: requeued ({reason}, restart {})",
            next.job_id, next.restarts
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

/// Requeue the given jids (looked up fresh in running/) after their VM
/// was reaped.
pub(in crate::monitor::monitor) async fn requeue_jids_after_reap(
    store: &JobStorage,
    jids: &[String],
    reason: &str,
) -> Result<(), MonitorError> {
    if jids.is_empty() {
        return Ok(());
    }
    let running: BTreeMap<String, Job> = store
        .list_jobs("running", 0)
        .await?
        .into_iter()
        .map(|j| (j.job_id.clone(), j))
        .collect();
    for jid in jids {
        let Some(job) = running.get(jid).cloned() else {
            continue;
        };
        let mut job = job;
        requeue(store, &mut job, reason).await?;
    }
    Ok(())
}
