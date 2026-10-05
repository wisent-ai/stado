//! The move no VM reaper can make: a job stranded on an operator-owned
//! local@ host whose capacity broadcast went stale, requeued on proof of
//! death alone and without ever touching the host.

use crate::models::Job;
use crate::monitor::heartbeat_guard as hg;
use crate::queue::JobStorage;

use super::super::{log, MonitorError};
use super::restart::requeue;

/// Requeue a job orphaned on a stale non-cloud local@ agent.
///
/// reap_dead_agents only iterates GCP provider VMs, so a local@<host>
/// that is NOT a wisent-agent-* cloud VM (e.g. an on-premises GPU box) is
/// never reaped there. In check_running_jobs the agent_live block is skipped
/// once that agent's capacity broadcast goes stale, and the
/// is_cloud_agent_name block does not match, so control would fall to a bare
/// `continue` and the job would wedge in running/ forever. Leave the job alone
/// only if it is alive by its worker's promise (or wrote a pulse or
/// checkpoint after it, or carries no promise) or its command self-terminates
/// the agent (kill is the success condition); otherwise requeue it. No VM
/// delete — the local host is operator-owned and must not be touched.
pub(in crate::monitor::monitor) async fn requeue_dead_local_host_orphan(
    store: &JobStorage,
    job: &mut Job,
) -> Result<(), MonitorError> {
    if hg::job_liveness(store, job, chrono::Utc::now())
        .await
        .alive()
    {
        return Ok(());
    }
    if hg::finalize_if_self_terminating(store, job, &log).await? {
        return Ok(());
    }
    requeue(
        store,
        job,
        "local agent capacity stale & job heartbeat stale (dead local host orphan)",
    )
    .await
    .map(|_| ())
}
