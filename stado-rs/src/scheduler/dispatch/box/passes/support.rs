//! Shared Boat-dispatch state: the read of a prefix's Boat jobs, the layer
//! error type, and the failure/relinquish helpers every pass in this tree
//! calls.

use futures::{StreamExt, TryStreamExt};

use crate::models::{job_state, Job};
use crate::providers::r#box::BoxError;
use crate::queue::leases::{LeaseError, LeaseState, ProviderLease, ProviderLeaseStore};
use crate::queue::{JobStorage, StorageError};

use super::super::runtime::now_iso;

/// Every Boat job under `prefix`, each read once, with the store's measured
/// worker count in flight. A tick advances every Boat job it finds, so no
/// window is chosen; a job between two prefixes reads as absent and is
/// picked up by the next tick.
pub(super) async fn boat_jobs(store: &JobStorage, prefix: &str) -> Result<Vec<Job>, StorageError> {
    let ids = store.list_job_ids(prefix).await?;
    let workers = crate::queue::migrations::bulk_workers()?;
    let read: Vec<Option<Job>> = futures::stream::iter(&ids)
        .map(|id| store.read_job(prefix, id))
        .buffered(workers)
        .try_collect()
        .await?;
    Ok(read
        .into_iter()
        .flatten()
        .filter(|job| crate::capabilities::ProviderId::Boat.matches(&job.provider))
        .collect())
}

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
            BoxDispatchError::Box(_) => "BoatError",
            BoxDispatchError::Lease(_) => "LeaseError",
            BoxDispatchError::Storage(_) => "StorageError",
            BoxDispatchError::Value(_) => "ValueError",
            BoxDispatchError::Runtime(_) => "RuntimeError",
        }
    }
}

/// One failed job of a Boat pass, said on stderr.
pub(super) fn log_failure(job_id: &str, exc: &BoxDispatchError) {
    let text: String = exc.to_string().replace(['\r', '\n'], " ");
    eprintln!("[boat] job={job_id} {}: {text}", exc.type_name());
}

/// Python `_fail_queued`.
pub(super) async fn fail_queued(
    store: &JobStorage,
    job: &mut Job,
    message: &str,
) -> Result<(), BoxDispatchError> {
    job.state = job_state::FAILED.to_string();
    job.error = Some(message.to_string());
    job.failed_at = Some(now_iso());
    store.move_job(job, "queue", "failed").await?;
    Ok(())
}

/// Python `_relinquish`: best-effort owner release. A release that fails
/// does not fail the pass, but it is said: the lease then stays held until
/// it expires, and a provider that admits nothing in the meantime needs that
/// line to be explained.
pub(super) async fn relinquish(leases: &ProviderLeaseStore, lease: Option<ProviderLease>) {
    let Some(mut lease) = lease else { return };
    // A lease whose owner is already gone has nothing to release. One whose
    // stored owner fields no longer parse cannot be released either, and that
    // is said like any other failed release: it stays held until it expires.
    match lease.owner_expired() {
        Ok(true) => return,
        Ok(false) => {}
        Err(error) => {
            let text = error.to_string().replace(['\r', '\n'], " ");
            eprintln!(
                "[boat] provider lease owned by {} was not released: its owner fields do not parse: {text}",
                lease.owner_id
            );
            return;
        }
    }
    let (owner, token) = (lease.owner_id.clone(), lease.fence_token.clone());
    let result: Result<(), BoxDispatchError> = async {
        lease.relinquish(&owner, &token)?;
        let version = lease.version.clone();
        leases.save(lease, &version).await?;
        Ok(())
    }
    .await;
    if let Err(error) = result {
        let text = error.to_string().replace(['\r', '\n'], " ");
        eprintln!("[boat] provider lease owned by {owner} was not released: {text}");
    }
}
