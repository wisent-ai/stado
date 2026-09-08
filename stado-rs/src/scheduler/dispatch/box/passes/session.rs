//! The three entry points that mint a unique owner id per invocation before
//! touching a fence: Python `cancel_box_job`, `cancel_box_for_legacy_move`
//! and `run_box_tick`.

use uuid::Uuid;

use crate::models::{job_state, Job};
use crate::providers::r#box::BoxProvider;
use crate::queue::leases::{LeaseState, ProviderLeaseStore};
use crate::queue::JobStorage;

use super::super::runtime::BoxRuntime;
use super::admit::dispatch_box_jobs;
use super::reconcile::reconcile_box_jobs;
use super::support::{relinquish, BoxDispatchError, OWNER_TTL_SECONDS};

/// Python `cancel_box_job`: cancel the process or prompt, then release
/// the fenced resource.
pub async fn cancel_box_job(
    store: &JobStorage,
    provider: &BoxProvider,
    job: &mut Job,
    owner_id: &str,
) -> Result<(), BoxDispatchError> {
    let leases = ProviderLeaseStore::new(store.clone());
    let session_owner = format!("{owner_id}:{}", Uuid::new_v4().simple());
    let ttl = if job.box_ttl_seconds != 0 {
        job.box_ttl_seconds
    } else {
        provider.ttl_seconds
    };
    let mut lease = leases
        .acquire(
            &job.job_id,
            crate::capabilities::ProviderId::Box.as_str(),
            &session_owner,
            OWNER_TTL_SECONDS,
            ttl,
        )
        .await?;
    let runtime = BoxRuntime::new(store, provider, &leases);
    let result = runtime.cancel(job, &mut lease).await;
    relinquish(&leases, Some(lease)).await;
    result
}

/// Python `cancel_box_for_legacy_move`: the fenced bridge used by
/// `BoxProvider.delete_instance` when a running/ job still references the
/// box. NOTE: Python does NOT relinquish here (no finally) — the owner
/// TTL lapses on its own.
pub async fn cancel_box_for_legacy_move(
    store: &JobStorage,
    provider: &BoxProvider,
    job: &mut Job,
    owner_id: &str,
) -> Result<(), BoxDispatchError> {
    let leases = ProviderLeaseStore::new(store.clone());
    let session_owner = format!("{owner_id}:{}", Uuid::new_v4().simple());
    let ttl = if job.box_ttl_seconds != 0 {
        job.box_ttl_seconds
    } else {
        provider.ttl_seconds
    };
    let mut lease = leases
        .acquire(
            &job.job_id,
            crate::capabilities::ProviderId::Box.as_str(),
            &session_owner,
            OWNER_TTL_SECONDS,
            ttl,
        )
        .await?;
    let runtime = BoxRuntime::new(store, provider, &leases);
    runtime.interrupt(job, &lease).await?;
    lease.result_state = job_state::FAILED.to_string();
    lease.last_error = "cancelled".to_string();
    let terminal = [
        LeaseState::Failed.as_str(),
        LeaseState::Releasing.as_str(),
        LeaseState::Released.as_str(),
    ];
    if !terminal.contains(&lease.state.as_str()) {
        let token = lease.fence_token.clone();
        lease.transition(LeaseState::Failed, &session_owner, &token)?;
        let version = lease.version.clone();
        lease = leases.save(lease, &version).await?;
    }
    if lease.state == LeaseState::Failed.as_str() {
        let token = lease.fence_token.clone();
        lease.transition(LeaseState::Releasing, &session_owner, &token)?;
        let version = lease.version.clone();
        lease = leases.save(lease, &version).await?;
    }
    if lease.state == LeaseState::Releasing.as_str() {
        provider.release_box(&lease.provider_resource_id).await?;
        let token = lease.fence_token.clone();
        lease.transition(LeaseState::Released, &session_owner, &token)?;
        let version = lease.version.clone();
        leases.save(lease, &version).await?;
    }
    Ok(())
}

/// Python `run_box_tick`: unique owner per invocation; reconcile before
/// allocating.
pub async fn run_box_tick(
    store: &JobStorage,
    provider: &BoxProvider,
    owner_id: &str,
) -> Result<i64, BoxDispatchError> {
    let session_owner = format!("{owner_id}:{}", Uuid::new_v4().simple());
    let reconciled = reconcile_box_jobs(store, provider, &session_owner).await?;
    let dispatched = dispatch_box_jobs(store, provider, &session_owner).await?;
    Ok(reconciled + dispatched)
}
