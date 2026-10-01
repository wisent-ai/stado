//! A platform build pinned to a host that has gone silent is moved to another
//! host of the same platform.
//!
//! A build job is hard-pinned to the builder it was placed on, and the
//! queue's reaper never releases a hard pin, so a job queued on a host that
//! stopped publishing capacity would wait for it forever — every release
//! behind a host with a full disk while other hosts of the fleet sit idle. A
//! job that is still queued (no host has
//! started it, so nothing is lost) and whose pinned host has fallen out of
//! the live capacity set is cancelled here; the build then records the
//! platform failed and places a replacement among the hosts that do publish,
//! the same placement a rebuild after any failure uses.

use crate::cli::CmdError;
use crate::models::job_state;
use crate::queue::capacity;
use crate::queue::storage::JobStorage;

/// The host a still-queued job is pinned to, when that host no longer
/// publishes capacity inside the fleet's own liveness horizon
/// ([`capacity::CAPACITY_STALE_SECONDS`]).
async fn silent_pinned_host(store: &JobStorage, job_id: &str) -> Result<Option<String>, CmdError> {
    let Some(job) = store
        .read_job("queue", job_id)
        .await
        .map_err(|error| CmdError::click(error.to_string()))?
    else {
        return Ok(None);
    };
    if job.state != job_state::QUEUED || job.pinned_host.is_empty() {
        return Ok(None);
    }
    let live = capacity::read_consumer_capacity(store)
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    let alive = live
        .keys()
        .any(|consumer| consumer.eq_ignore_ascii_case(&job.pinned_host));
    Ok((!alive).then_some(job.pinned_host))
}

/// Cancel a still-queued build job whose pinned host is silent, and say why,
/// so the caller records the platform failed and places it again elsewhere.
/// `None` when the job is running, terminal, unpinned, or its host is live,
/// and when a host claims it between the read and the cancel: a claimed job
/// is left to run.
pub(crate) async fn release_silent_placement(
    store: &JobStorage,
    job_id: &str,
) -> Result<Option<String>, CmdError> {
    let Some(host) = silent_pinned_host(store, job_id).await? else {
        return Ok(None);
    };
    if !crate::cli::work::cancel::cancel_queued_in_store(store, job_id).await? {
        return Ok(None);
    }
    Ok(Some(format!(
        "build job {job_id} was queued on {host}, which publishes no capacity within {}s; \
         it was cancelled and the platform is placed on another host",
        capacity::CAPACITY_STALE_SECONDS
    )))
}
