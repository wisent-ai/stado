//! The admit half of the tick: Python `dispatch_box_jobs` plus the
//! resource-provisioning body it runs under an acquired lease.

use crate::models::{job_state, Job};
use crate::providers::r#box::BoxProvider;
use crate::queue::leases::{LeaseState, ProviderLease, ProviderLeaseStore};
use crate::queue::JobStorage;

use super::super::runtime::now_iso;
use super::support::{
    fail_queued, log_failure, relinquish, BoxDispatchError, OWNER_TTL_SECONDS, QUEUE_SCAN_BUDGET,
    QUEUE_SCAN_CAP,
};

/// Python `dispatch_box_jobs`: admit pinned queued jobs and allocate
/// available Box capacity.
pub async fn dispatch_box_jobs(
    store: &JobStorage,
    provider: &BoxProvider,
    owner_id: &str,
) -> Result<i64, BoxDispatchError> {
    // Maintenance-mode gate (queue::control). Box dispatch is the OTHER
    // queue/ -> running/ mover in the coordinator tick, so a pause has to
    // stop it too or `stado queue drain --wait` would watch running/ grow
    // while it waits. Only the ADMIT half is gated: run_box_tick's
    // reconcile pass still drives already-leased boxes to completion —
    // the same asymmetry the local agent has between advance_slot and its
    // claim scan.
    let queue_control = crate::queue::control::read(store).await?;
    if queue_control.paused {
        eprintln!(
            "[box] queue paused ({}); admitting no new jobs",
            queue_control.pause_summary()
        );
        return Ok(i64::default());
    }
    let leases = ProviderLeaseStore::new(store.clone());
    let mut scheduled: i64 = 0;
    // The window has to count BOX jobs, not queued jobs: a queue whose oldest
    // twenty-five entries belong to other providers would otherwise hand this
    // tick nothing it can dispatch, on every tick, while a Box job waits just
    // past the window.
    for mut job in store
        .list_claimable_jobs(
            "queue",
            &crate::queue::listing::JobScan {
                want: QUEUE_SCAN_CAP,
                scan_budget: QUEUE_SCAN_BUDGET,
                max_gpu_mem_gb: i64::MAX,
                eligible: &|job| crate::capabilities::ProviderId::Box.matches(&job.provider),
                // Dispatch wants reachability, so it takes the shared
                // rotation: work past one tick's window is picked up by the
                // next tick instead of never.
                from_head: false,
            },
        )
        .await?
    {
        if !job.secret_env.is_empty() {
            fail_queued(
                store,
                &mut job,
                "Box jobs do not support workload secret references",
            )
            .await?;
            continue;
        }
        if !job.pin_to_provider {
            fail_queued(store, &mut job, "Box jobs must set pin_to_provider=true").await?;
            continue;
        }
        let decision = provider.admit(&job);
        if !decision.accepted {
            fail_queued(store, &mut job, &decision.reasons.join("; ")).await?;
            continue;
        }
        let resource_ttl = if job.box_ttl_seconds != 0 {
            job.box_ttl_seconds
        } else {
            provider.ttl_seconds
        };
        let mut lease: Option<ProviderLease> = None;
        let mut resource_recorded = false;
        let outcome: Result<bool, BoxDispatchError> = async {
            let mut acquired = leases
                .acquire(
                    &job.job_id,
                    crate::capabilities::ProviderId::Box.as_str(),
                    owner_id,
                    OWNER_TTL_SECONDS,
                    resource_ttl,
                )
                .await?;
            let scheduled = provision_and_move(
                store,
                provider,
                &leases,
                &mut job,
                &mut acquired,
                resource_ttl,
                &mut resource_recorded,
            )
            .await?;
            lease = Some(acquired);
            Ok(scheduled)
        }
        .await;
        match outcome {
            Ok(did_schedule) => {
                relinquish(&leases, lease).await;
                if did_schedule {
                    scheduled += 1;
                }
            }
            Err(err) if err.is_conflict() => {
                relinquish(&leases, lease).await;
            }
            Err(err) => {
                log_failure(&job.job_id, &err);
                // Python's except-handler failures propagate out of the
                // loop, but the finally-block relinquish runs first.
                let handler: Result<(), BoxDispatchError> = async {
                    if !resource_recorded {
                        if let Some(l) = lease.as_mut() {
                            if l.state == LeaseState::Allocating.as_str() {
                                l.last_error =
                                    "Box allocation outcome is unknown; resource TTL remains the bound"
                                        .to_string();
                                l.result_state = job_state::FAILED.to_string();
                                let (owner, token) =
                                    (l.owner_id.clone(), l.fence_token.clone());
                                l.transition(LeaseState::Failed, &owner, &token)?;
                                let version = l.version.clone();
                                *l = leases.save(l.clone(), &version).await?;
                            }
                        }
                        fail_queued(store, &mut job, &err.to_string()).await?;
                    }
                    Ok(())
                }
                .await;
                relinquish(&leases, lease).await;
                handler?;
            }
        }
    }
    Ok(scheduled)
}

/// The resource-provisioning half of Python's `dispatch_box_jobs` loop
/// body (everything inside the try after `acquire`).
#[allow(clippy::too_many_arguments)]
async fn provision_and_move(
    store: &JobStorage,
    provider: &BoxProvider,
    leases: &ProviderLeaseStore,
    job: &mut Job,
    lease: &mut ProviderLease,
    resource_ttl: i64,
    resource_recorded: &mut bool,
) -> Result<bool, BoxDispatchError> {
    if !lease.provider_resource_id.is_empty() {
        *resource_recorded = true;
        let terminal = [
            LeaseState::Failed.as_str(),
            LeaseState::Releasing.as_str(),
            LeaseState::Released.as_str(),
        ];
        if terminal.contains(&lease.state.as_str()) {
            fail_queued(store, job, &format!("Box lease is already {}", lease.state)).await?;
            return Ok(false);
        }
        job.instance_ref = Some(lease.provider_resource_id.clone());
    } else {
        let created = provider.create_box(Some(resource_ttl)).await?;
        lease.provider_resource_id = created.box_id.clone();
        let (owner, token) = (lease.owner_id.clone(), lease.fence_token.clone());
        lease.transition(LeaseState::Provisioning, &owner, &token)?;
        let version = lease.version.clone();
        *lease = leases.save(lease.clone(), &version).await?;
        *resource_recorded = true;
        job.instance_ref = Some(created.box_id);
    }
    job.state = job_state::RUNNING.to_string();
    if job.started_at.as_deref().is_none_or(str::is_empty) {
        job.started_at = Some(now_iso());
    }
    store.move_job(job, "queue", "running").await?;
    Ok(true)
}
