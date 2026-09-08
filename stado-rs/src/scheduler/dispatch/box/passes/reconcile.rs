//! The reconcile half of the tick: Python `_box_state`, `_reconcile_one` and
//! `reconcile_box_jobs`, advancing every persisted lease state exactly once.

use chrono::Utc;

use crate::models::Job;
use crate::providers::r#box::{BoxError, BoxProvider};
use crate::queue::leases::{LeaseState, ProviderLease, ProviderLeaseStore};
use crate::queue::JobStorage;

use super::super::runtime::{parse_iso, BoxRuntime};
use super::support::{
    log_failure, relinquish, renewed_state, BoxDispatchError, OWNER_TTL_SECONDS, READY_BOX_STATES,
    START_RECOVERY_SECONDS,
};

/// Python `_box_state`: "gone" maps a 404.
async fn box_state(provider: &BoxProvider, lease: &ProviderLease) -> Result<String, BoxError> {
    match provider.client.get_box(&lease.provider_resource_id).await {
        Ok(info) => Ok(info.state),
        Err(BoxError::Api(api)) if api.status == 404 => Ok("gone".to_string()),
        Err(err) => Err(err),
    }
}

/// Python `_reconcile_one`. Returns true when the lease state advanced
/// (or the lease reached a terminal disposition) this tick.
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
            .fail(job, lease, "Box lease has no provider resource", true)
            .await?;
        return Ok(true);
    }
    let box_state = box_state(provider, lease).await?;
    if box_state == "gone" || box_state == "archived" {
        runtime
            .fail(
                job,
                lease,
                &format!("Box became {box_state} before completion"),
                true,
            )
            .await?;
        return Ok(true);
    }
    if box_state == "error" {
        runtime
            .fail(job, lease, "Box entered error state", false)
            .await?;
        return Ok(true);
    }
    let ttl = if job.box_ttl_seconds != 0 {
        job.box_ttl_seconds
    } else {
        provider.ttl_seconds
    };
    if renewed_state(&lease.state) {
        provider
            .renew_box(&lease.provider_resource_id, Some(ttl))
            .await?;
        let (owner, token) = (lease.owner_id.clone(), lease.fence_token.clone());
        lease.renew_resource(&owner, &token, ttl)?;
        lease.renew_owner(&owner, &token, OWNER_TTL_SECONDS)?;
        let version = lease.version.clone();
        *lease = leases.save(lease.clone(), &version).await?;
    }
    if lease.state == LeaseState::Provisioning.as_str() {
        if !READY_BOX_STATES.contains(&box_state.as_str()) {
            return Ok(false);
        }
        let (owner, token) = (lease.owner_id.clone(), lease.fence_token.clone());
        lease.transition(LeaseState::Ready, &owner, &token)?;
        let version = lease.version.clone();
        *lease = leases.save(lease.clone(), &version).await?;
    }
    if lease.state == LeaseState::Ready.as_str() || lease.state == LeaseState::Starting.as_str() {
        match runtime.start(job, lease).await {
            Ok(started) => return Ok(started),
            Err(err) => {
                if !lease.operation_started_at.is_empty() {
                    if let Some(started) = parse_iso(&lease.operation_started_at) {
                        let age = (Utc::now() - started).num_seconds();
                        if age >= START_RECOVERY_SECONDS {
                            runtime
                                .fail(
                                    job,
                                    lease,
                                    "Box start did not recover before deadline",
                                    false,
                                )
                                .await?;
                            return Ok(true);
                        }
                    }
                }
                return Err(err);
            }
        }
    }
    if lease.state == LeaseState::Running.as_str() {
        return runtime.reconcile_running(job, lease).await;
    }
    if lease.state == LeaseState::Allocating.as_str() {
        runtime
            .fail(job, lease, "Box allocation did not record a resource", true)
            .await?;
        return Ok(true);
    }
    Err(BoxDispatchError::runtime(format!(
        "unhandled Box lease state {}",
        lease.state
    )))
}

/// Python `reconcile_box_jobs`: advance every persisted lease state
/// without duplicating mutations.
pub async fn reconcile_box_jobs(
    store: &JobStorage,
    provider: &BoxProvider,
    owner_id: &str,
) -> Result<i64, BoxDispatchError> {
    let leases = ProviderLeaseStore::new(store.clone());
    let runtime = BoxRuntime::new(store, provider, &leases);
    let mut changed: i64 = 0;
    for mut job in store.list_jobs("running", 0).await? {
        if !crate::capabilities::ProviderId::Box.matches(&job.provider) {
            continue;
        }
        let ttl = if job.box_ttl_seconds != 0 {
            job.box_ttl_seconds
        } else {
            provider.ttl_seconds
        };
        let mut lease = match leases
            .acquire(
                &job.job_id,
                crate::capabilities::ProviderId::Box.as_str(),
                owner_id,
                OWNER_TTL_SECONDS,
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
                Ok(true) => changed += 1,
                Ok(false) => {}
                Err(err) if err.is_conflict() => {}
                Err(err) => log_failure(&job.job_id, &err),
            }
        }
        relinquish(&leases, lease).await;
    }
    Ok(changed)
}
