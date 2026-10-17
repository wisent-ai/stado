//! The admit half of the tick: Boat-pinned queued jobs are admitted and each
//! gets its sandbox under an acquired lease.

use crate::models::{job_state, Job};
use crate::providers::r#box::BoxProvider;
use crate::queue::leases::{LeaseState, ProviderLease, ProviderLeaseStore};
use crate::queue::JobStorage;

use super::super::runtime::now_iso;
use super::support::{boat_jobs, fail_queued, log_failure, relinquish, BoxDispatchError};

/// Admit the queued Boat-pinned jobs, highest priority and oldest first, and
/// create a sandbox for each. Returns how many jobs moved to running.
pub async fn dispatch_box_jobs(
    store: &JobStorage,
    provider: &BoxProvider,
    owner_id: &str,
) -> Result<i64, BoxDispatchError> {
    // Maintenance-mode gate (queue::control). Boat dispatch is the OTHER
    // queue/ -> running/ mover in the coordinator tick, so a pause has to
    // stop it too or `stado queue drain` would watch running/ grow. Only the
    // ADMIT half is gated: run_box_tick's reconcile pass still drives
    // already-leased sandboxes to completion — the same asymmetry the local
    // agent has between advance_slot and its claim scan.
    let queue_control = crate::queue::control::read(store).await?;
    if queue_control.paused {
        eprintln!(
            "[boat] queue paused ({}); admitting no new jobs",
            queue_control.pause_summary()
        );
        return count(&[]);
    }
    let leases = ProviderLeaseStore::new(store.clone());
    let mut scheduled: Vec<String> = Vec::new();
    let mut queued = boat_jobs(store, "queue").await?;
    queued.sort_by(|a, b| {
        b.priority
            .cmp(&a.priority)
            .then_with(|| a.created_at.cmp(&b.created_at))
    });
    for mut job in queued {
        if !job.secret_env.is_empty() {
            fail_queued(
                store,
                &mut job,
                "a Boat job cannot carry workload secret references",
            )
            .await?;
            continue;
        }
        if !job.pin_to_provider {
            fail_queued(store, &mut job, "a Boat job must set pin_to_provider=true").await?;
            continue;
        }
        if job.box_ttl_seconds.is_negative() {
            let reason = format!(
                "box_ttl_seconds is {}; a Boat sandbox's lifetime is a positive number of \
                 seconds, or unset for the deployment's boat.ttl_seconds",
                job.box_ttl_seconds
            );
            fail_queued(store, &mut job, &reason).await?;
            continue;
        }
        let decision = provider.admit(&job);
        if !decision.accepted {
            fail_queued(store, &mut job, &decision.reasons.join("; ")).await?;
            continue;
        }
        let resource_ttl = provider.ttl_for(&job);
        let mut lease: Option<ProviderLease> = None;
        let mut resource_recorded = false;
        let outcome: Result<bool, BoxDispatchError> = async {
            let mut acquired = leases
                .acquire(
                    &job.job_id,
                    crate::capabilities::ProviderId::Boat.as_str(),
                    owner_id,
                    resource_ttl,
                )
                .await?;
            let moved = provision_and_move(
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
            Ok(moved)
        }
        .await;
        match outcome {
            Ok(moved) => {
                relinquish(&leases, lease).await;
                if moved {
                    scheduled.push(job.job_id.clone());
                }
            }
            Err(err) if err.is_conflict() => {
                relinquish(&leases, lease).await;
            }
            Err(err) => {
                log_failure(&job.job_id, &err);
                // The failure handling below propagates out of the loop, but
                // the lease is relinquished first.
                let handler: Result<(), BoxDispatchError> = async {
                    if !resource_recorded {
                        if let Some(l) = lease.as_mut() {
                            if l.state == LeaseState::Allocating.as_str() {
                                l.last_error = "whether Boat created the sandbox is unknown; \
                                                the sandbox lifetime remains the bound"
                                    .to_string();
                                l.result_state = job_state::FAILED.to_string();
                                let (owner, token) = (l.owner_id.clone(), l.fence_token.clone());
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
    count(&scheduled)
}

/// How many jobs a pass moved, as the tick counts them.
fn count(moved: &[String]) -> Result<i64, BoxDispatchError> {
    i64::try_from(moved.len()).map_err(|error| {
        BoxDispatchError::runtime(format!(
            "{} admitted Boat jobs do not fit a count: {error}",
            moved.len()
        ))
    })
}

/// Everything after the lease is acquired: the sandbox is created (or the
/// one the lease already records is reused) and the job moves to running.
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
            fail_queued(
                store,
                job,
                &format!("the Boat lease is already {}", lease.state),
            )
            .await?;
            return Ok(false);
        }
        job.instance_ref = Some(lease.provider_resource_id.clone());
    } else {
        let created = provider.create_box(resource_ttl).await?;
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
