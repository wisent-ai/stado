//! The reconcile half of the tick: advancing every persisted lease state of
//! a Boat-pinned job exactly once.

use crate::models::Job;
use crate::providers::r#box::types::HTTP_NOT_FOUND;
use crate::providers::r#box::{BoxError, BoxProvider, BoxState};
use crate::queue::leases::{LeaseState, ProviderLease, ProviderLeaseStore};
use crate::queue::JobStorage;

use super::super::runtime::BoxRuntime;
use super::support::{boat_jobs, log_failure, relinquish, renewed_state, BoxDispatchError};

/// The sandbox's lifecycle, `None` when Boat no longer holds it.
async fn box_state(
    provider: &BoxProvider,
    lease: &ProviderLease,
) -> Result<Option<BoxState>, BoxError> {
    match provider.client.get_box(&lease.provider_resource_id).await {
        Ok(info) => Ok(Some(info.lifecycle())),
        Err(BoxError::Api(api)) if api.status == HTTP_NOT_FOUND => Ok(None),
        Err(err) => Err(err),
    }
}

/// Whether a start error will be the same on every later attempt.
fn start_refused_for_good(err: &BoxDispatchError) -> bool {
    match err {
        BoxDispatchError::Box(BoxError::Api(api)) => !api.retryable,
        BoxDispatchError::Box(BoxError::Value(_) | BoxError::Configuration(_)) => true,
        BoxDispatchError::Value(_) => true,
        _ => false,
    }
}

/// Advance one lease. Returns true when the lease state advanced (or the
/// lease reached a terminal disposition) this tick.
async fn reconcile_one(
    provider: &BoxProvider,
    runtime: &BoxRuntime<'_>,
    leases: &ProviderLeaseStore,
    job: &mut Job,
    lease: &mut ProviderLease,
) -> Result<bool, BoxDispatchError> {
    let state = lease.state.parse::<LeaseState>()?;
    if matches!(
        state,
        LeaseState::Collecting | LeaseState::Failed | LeaseState::Releasing | LeaseState::Released
    ) {
        runtime.resume_terminal(job, lease).await?;
        return Ok(true);
    }
    if lease.provider_resource_id.is_empty() {
        runtime
            .fail(job, lease, "Boat lease has no sandbox", true)
            .await?;
        return Ok(true);
    }
    let box_state = match box_state(provider, lease).await? {
        None => {
            runtime
                .fail(
                    job,
                    lease,
                    "Boat no longer holds the sandbox before completion",
                    true,
                )
                .await?;
            return Ok(true);
        }
        Some(BoxState::Archived) => {
            runtime
                .fail(
                    job,
                    lease,
                    "the Boat sandbox stopped before completion",
                    true,
                )
                .await?;
            return Ok(true);
        }
        Some(BoxState::Cancelled) => {
            runtime
                .fail(
                    job,
                    lease,
                    "Boat cancelled the sandbox: no machine of its size was ready",
                    true,
                )
                .await?;
            return Ok(true);
        }
        Some(BoxState::Error) => {
            runtime
                .fail(
                    job,
                    lease,
                    "the Boat sandbox entered its error state",
                    false,
                )
                .await?;
            return Ok(true);
        }
        Some(state) => state,
    };
    let ttl = provider.ttl_for(job);
    if renewed_state(&lease.state) {
        provider.renew_box(&lease.provider_resource_id, ttl).await?;
        let (owner, token) = (lease.owner_id.clone(), lease.fence_token.clone());
        lease.renew_resource(&owner, &token, ttl)?;
        lease.renew_owner(&owner, &token)?;
        let version = lease.version.clone();
        *lease = leases.save(lease.clone(), &version).await?;
    }
    if lease.state == LeaseState::Provisioning.as_str() {
        if !box_state.serving() {
            return Ok(false);
        }
        let (owner, token) = (lease.owner_id.clone(), lease.fence_token.clone());
        lease.transition(LeaseState::Ready, &owner, &token)?;
        let version = lease.version.clone();
        *lease = leases.save(lease.clone(), &version).await?;
    }
    if lease.state == LeaseState::Ready.as_str() || lease.state == LeaseState::Starting.as_str() {
        // A start the sandbox refuses for good (an API error it marks not
        // retryable, or a request that is invalid as written) fails the job
        // now with that refusal; a transport failure or a retryable refusal
        // is tried again next tick and logged each time.
        return match runtime.start(job, lease).await {
            Ok(started) => Ok(started),
            Err(err) if start_refused_for_good(&err) => {
                let reason = format!("Boat refused the start: {err}");
                runtime.fail(job, lease, &reason, false).await?;
                Ok(true)
            }
            Err(err) => Err(err),
        };
    }
    if lease.state == LeaseState::Running.as_str() {
        return runtime.reconcile_running(job, lease).await;
    }
    if lease.state == LeaseState::Allocating.as_str() {
        runtime
            .fail(
                job,
                lease,
                "the Boat allocation did not record a sandbox",
                true,
            )
            .await?;
        return Ok(true);
    }
    Err(BoxDispatchError::runtime(format!(
        "unhandled Boat lease state {}",
        lease.state
    )))
}

/// Advance every persisted lease state of a running Boat-pinned job without
/// duplicating mutations. Returns how many leases advanced.
pub async fn reconcile_box_jobs(
    store: &JobStorage,
    provider: &BoxProvider,
    owner_id: &str,
) -> Result<i64, BoxDispatchError> {
    let leases = ProviderLeaseStore::new(store.clone());
    let runtime = BoxRuntime::new(store, provider, &leases);
    let mut advanced: Vec<String> = Vec::new();
    for mut job in boat_jobs(store, "running").await? {
        let ttl = provider.ttl_for(&job);
        let mut lease = match leases
            .acquire(
                &job.job_id,
                crate::capabilities::ProviderId::Boat.as_str(),
                owner_id,
                ttl,
            )
            .await
        {
            Ok(lease) => Some(lease),
            Err(err) if err.is_conflict() => continue,
            Err(err) => {
                log_failure(&job.job_id, &err.into());
                continue;
            }
        };
        if let Some(l) = lease.as_mut() {
            match reconcile_one(provider, &runtime, &leases, &mut job, l).await {
                Ok(true) => advanced.push(job.job_id.clone()),
                Ok(false) => {}
                Err(err) if err.is_conflict() => {}
                Err(err) => log_failure(&job.job_id, &err),
            }
        }
        relinquish(&leases, lease).await;
    }
    i64::try_from(advanced.len()).map_err(|error| {
        BoxDispatchError::runtime(format!(
            "{} advanced leases do not fit a count: {error}",
            advanced.len()
        ))
    })
}
