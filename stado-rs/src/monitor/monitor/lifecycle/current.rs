//! The fresh read every lifecycle move decides from: the running document as
//! it stands right now, the version that move must pin, and the lease test
//! that says the worker still owns the job.

use chrono::Utc;

use crate::models::{job_state, Job};
use crate::monitor::heartbeat_guard as hg;
use crate::queue::{JobStorage, StorageError};

use super::super::MonitorError;

/// The running document as it stands right now, together with the version a
/// lifecycle move must pin.
///
/// Every requeue below is a liveness verdict reached from a tick-start
/// listing, and the worker's own lease renewal rewrites this document every
/// [`crate::providers::local::slots::HEARTBEAT_INTERVAL_S`]. Deciding from
/// the stale copy and moving unconditionally is what let a live execution be
/// requeued and started a second time; pinning the fresh version makes the
/// renewal win the race.
pub(super) async fn current_running(
    store: &JobStorage,
    job_id: &str,
) -> Result<Option<(Job, String)>, MonitorError> {
    let Some(versioned) = store
        .read_text_versioned(&format!("running/{job_id}.json"))
        .await?
    else {
        return Ok(None);
    };
    let current = Job::from_json(&versioned.content).map_err(StorageError::from)?;
    if current.state != job_state::RUNNING {
        return Ok(None);
    }
    Ok(Some((current, versioned.version)))
}
/// Whether the worker lease in a freshly-read running document still owns the
/// job. Legacy jobs have no lease and therefore continue through the external
/// heartbeat/checkpoint checks that selected the requeue path.
pub(super) fn running_lease_live(job: &Job) -> bool {
    job.lease_expires_at
        .as_deref()
        .filter(|value| !value.is_empty())
        .and_then(hg::parse_iso_lenient)
        .is_some_and(|expires| expires > Utc::now())
}
