//! The external liveness signals a job carries beside its in-document
//! lease, read as ages so the expiry decision can compare them against one
//! TTL.

use chrono::Utc;

use crate::models::Job;
use crate::monitor::heartbeat_guard as hg;
use crate::queue::{JobStorage, StorageError};

/// Seconds since `status/<job_id>/heartbeat` was last written, or `None`
/// when no heartbeat blob exists (a worker that died before its first
/// write, or a just-requeued job whose status blobs were cleaned).
pub(super) async fn heartbeat_age_seconds(
    store: &JobStorage,
    job_id: &str,
    now: chrono::DateTime<Utc>,
) -> Result<Option<i64>, StorageError> {
    let path = format!("status/{job_id}/heartbeat");
    Ok(store
        .backend()
        .updated_at(&path)
        .await?
        .map(|updated| (now - updated).num_seconds()))
}

/// Seconds since the job was (last) started, per its `started_at` stamp.
pub(super) fn started_age_seconds(job: &Job, now: chrono::DateTime<Utc>) -> Option<i64> {
    let started = hg::parse_iso_lenient(job.started_at.as_deref().filter(|s| !s.is_empty())?)?;
    Some((now - started).num_seconds())
}
