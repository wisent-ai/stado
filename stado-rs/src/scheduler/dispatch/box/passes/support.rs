//! Shared Box-dispatch state: the per-tick bounds, the layer error type, and
//! the failure/relinquish helpers every pass in this tree calls.

use crate::models::{job_state, Job};
use crate::providers::r#box::BoxError;
use crate::queue::leases::{LeaseError, LeaseState, ProviderLease, ProviderLeaseStore};
use crate::queue::{JobStorage, StorageError};

use super::super::runtime::now_iso;

pub(super) const OWNER_TTL_SECONDS: i64 = 300;
pub(super) const QUEUE_SCAN_CAP: usize = 25;
/// Job documents one admit pass will read to find [`QUEUE_SCAN_CAP`] Box
/// jobs. The window counts Box work; this bounds what looking for it costs on
/// a queue dominated by other providers.
pub(super) const QUEUE_SCAN_BUDGET: usize = 2_000;
pub(super) const START_RECOVERY_SECONDS: i64 = 120;

/// Python `_READY_BOX_STATES`.
pub(super) const READY_BOX_STATES: [&str; 3] = ["ready", "idle", "running"];
/// Python `_RENEWED_STATES`.
pub(super) fn renewed_state(state: &str) -> bool {
    matches!(
        state,
        s if s == LeaseState::Provisioning.as_str()
            || s == LeaseState::Ready.as_str()
            || s == LeaseState::Starting.as_str()
            || s == LeaseState::Running.as_str()
    )
}

/// Box-dispatch layer error. Python raises `ValueError` for invalid lease
/// transitions / workload shapes, `RuntimeError` for state-machine
/// violations, and lets Box/lease/storage errors propagate.
#[derive(Debug, thiserror::Error)]
pub enum BoxDispatchError {
    /// Box API / transport / validation failures.
    #[error(transparent)]
    Box(#[from] BoxError),
    /// Fenced lease failures (conflicts, illegal transitions).
    #[error(transparent)]
    Lease(#[from] LeaseError),
    /// Queue storage failures.
    #[error(transparent)]
    Storage(#[from] StorageError),
    /// Python `ValueError`.
    #[error("{0}")]
    Value(String),
    /// Python `RuntimeError`.
    #[error("{0}")]
    Runtime(String),
}

impl BoxDispatchError {
    pub(crate) fn value(message: impl Into<String>) -> Self {
        BoxDispatchError::Value(message.into())
    }

    pub(crate) fn runtime(message: impl Into<String>) -> Self {
        BoxDispatchError::Runtime(message.into())
    }

    /// Python `except LeaseConflict`.
    pub(super) fn is_conflict(&self) -> bool {
        matches!(self, BoxDispatchError::Lease(err) if err.is_conflict())
    }

    /// Python `type(exc).__name__` for the failure log line.
    fn type_name(&self) -> &'static str {
        match self {
            BoxDispatchError::Box(_) => "BoxError",
            BoxDispatchError::Lease(_) => "LeaseError",
            BoxDispatchError::Storage(_) => "StorageError",
            BoxDispatchError::Value(_) => "ValueError",
            BoxDispatchError::Runtime(_) => "RuntimeError",
        }
    }
}

/// Python `_log_failure`.
pub(super) fn log_failure(job_id: &str, exc: &BoxDispatchError) {
    let text: String = exc
        .to_string()
        .replace(['\r', '\n'], " ")
        .chars()
        .take(512)
        .collect();
    eprintln!("[box] job={job_id} {}: {text}", exc.type_name());
}

/// Python `_fail_queued`.
pub(super) async fn fail_queued(
    store: &JobStorage,
    job: &mut Job,
    message: &str,
) -> Result<(), BoxDispatchError> {
    job.state = job_state::FAILED.to_string();
    job.error = Some(message.chars().take(512).collect());
    job.failed_at = Some(now_iso());
    store.move_job(job, "queue", "failed").await?;
    Ok(())
}

/// Python `_relinquish`: best-effort owner release; every failure is
/// swallowed.
pub(super) async fn relinquish(leases: &ProviderLeaseStore, lease: Option<ProviderLease>) {
    let Some(mut lease) = lease else { return };
    // A corrupt stored timestamp can no longer be renewed meaningfully;
    // treat it as expired (Python would raise out of owner_expired, but
    // the finally-block swallow is the operational intent).
    if lease.owner_expired().unwrap_or(true) {
        return;
    }
    let (owner, token) = (lease.owner_id.clone(), lease.fence_token.clone());
    let result: Result<(), BoxDispatchError> = async {
        lease.relinquish(&owner, &token)?;
        let version = lease.version.clone();
        leases.save(lease, &version).await?;
        Ok(())
    }
    .await;
    let _ = result;
}
