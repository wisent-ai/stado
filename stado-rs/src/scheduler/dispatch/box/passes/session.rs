//! The three entry points that mint a unique owner id per invocation before
//! touching a fence: `cancel_box_job`, `cancel_box_for_legacy_move` and
//! `run_box_tick`. Each holds an [`OwnerInvocation`] while it works, so its
//! leases are live exactly as long as the invocation runs.

use uuid::Uuid;

use crate::models::{job_state, Job};
use crate::providers::r#box::BoxProvider;
use crate::queue::leases::{LeaseState, OwnerInvocation, ProviderLeaseStore};
use crate::queue::JobStorage;

use super::super::runtime::BoxRuntime;
use super::admit::dispatch_box_jobs;
use super::reconcile::reconcile_box_jobs;
use super::support::{relinquish, BoxDispatchError};

/// Cancel the process or prompt in the job's Boat sandbox, then release the
/// fenced sandbox.
pub async fn cancel_box_job(
    store: &JobStorage,
    provider: &BoxProvider,
    job: &mut Job,
    owner_id: &str,
) -> Result<(), BoxDispatchError> {
    let leases = ProviderLeaseStore::new(store.clone());
    let session_owner = format!("{owner_id}:{}", Uuid::new_v4().simple());
    let _invocation = OwnerInvocation::begin(&session_owner);
    let ttl = provider.ttl_for(job);
    let mut lease = leases
        .acquire(
            &job.job_id,
            crate::capabilities::ProviderId::Boat.as_str(),
            &session_owner,
            ttl,
        )
        .await?;
    let runtime = BoxRuntime::new(store, provider, &leases);
    let result = runtime.cancel(job, &mut lease).await;
    relinquish(&leases, Some(lease)).await;
    result
}

/// The fenced bridge `BoxProvider::delete_instance` takes when a running/
/// job still references the sandbox. It does not relinquish: the lease ends
/// Released, and its owner is gone once this invocation returns.
pub async fn cancel_box_for_legacy_move(
    store: &JobStorage,
    provider: &BoxProvider,
    job: &mut Job,
    owner_id: &str,
) -> Result<(), BoxDispatchError> {
    let leases = ProviderLeaseStore::new(store.clone());
    let session_owner = format!("{owner_id}:{}", Uuid::new_v4().simple());
    let _invocation = OwnerInvocation::begin(&session_owner);
    let ttl = provider.ttl_for(job);
    let mut lease = leases
        .acquire(
            &job.job_id,
            crate::capabilities::ProviderId::Boat.as_str(),
            &session_owner,
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

/// One Boat tick: a unique owner per invocation; reconcile before
/// allocating.
pub async fn run_box_tick(
    store: &JobStorage,
    provider: &BoxProvider,
    owner_id: &str,
) -> Result<i64, BoxDispatchError> {
    let session_owner = format!("{owner_id}:{}", Uuid::new_v4().simple());
    let _invocation = OwnerInvocation::begin(&session_owner);
    let reconciled = reconcile_box_jobs(store, provider, &session_owner).await?;
    let dispatched = dispatch_box_jobs(store, provider, &session_owner).await?;
    Ok(reconciled + dispatched)
}
