use super::*;

mod handoff;
mod initial;
mod recheck;
mod writers;

pub(in crate::deploy::host_storage_reconcile) use handoff::*;
pub(in crate::deploy::host_storage_reconcile) use initial::*;
pub(in crate::deploy::host_storage_reconcile) use recheck::*;

use writers::*;

pub(super) async fn prepare_lifecycle_fence(
    storage_target: &crate::targets::ComputeTarget,
    transaction: &str,
    runner: &Runner,
    write_guard: &mut Option<std::fs::File>,
) -> Result<LifecycleFence, DeployError> {
    let mut fence = match read_fence(storage_target, transaction, runner).await? {
        Some(existing) => existing,
        None => initial_lifecycle_fence(storage_target, transaction, runner).await?,
    };
    if fence.schema != FENCE_SCHEMA || fence.transaction != transaction {
        return Err(DeployError(
            "durable lifecycle fence belongs to another transaction".to_string(),
        ));
    }
    refresh_resident_owner(storage_target, transaction, &mut fence, runner).await?;
    if fence.status == "fenced" {
        acquire_storage_write_fence(storage_target, transaction, &mut fence, write_guard, runner)
            .await?;
        return recheck_lifecycle_fence(storage_target, transaction, runner).await;
    }
    if fence.status != "preparing" {
        return Err(DeployError(format!(
            "lifecycle fence cannot prepare from {}",
            fence.status
        )));
    }

    let store = if fence.write_fence.is_some() {
        if !fence.queue.drained
            || fence
                .lease_acquisitions
                .iter()
                .any(|entry| entry.lease.is_none())
        {
            return Err(DeployError(
                "storage write fence preceded queue draining or lease acquisition".to_string(),
            ));
        }
        acquire_storage_write_fence(storage_target, transaction, &mut fence, write_guard, runner)
            .await?;
        None
    } else {
        Some(
            crate::queue::JobStorage::new()
                .await
                .map_err(|error| DeployError(format!("cannot open queue for fencing: {error}")))?,
        )
    };
    if let Some(store) = &store {
        const LEASE_TTL_SECONDS: u64 = 12 * 60 * 60;
        let subjects = fence
            .writers
            .iter()
            .map(|writer| format!("service:{}:{}", writer.target, writer.label))
            .collect::<Vec<_>>();
        for subject in subjects {
            let index = match fence
                .lease_acquisitions
                .iter()
                .position(|entry| entry.subject_id == subject)
            {
                Some(index) => index,
                None => {
                    fence.lease_acquisitions.push(LeaseAcquisition {
                        subject_id: subject.clone(),
                        status: "acquire_intent".to_string(),
                        lease: None,
                        released_lease: None,
                    });
                    write_fence(storage_target, transaction, &fence, runner).await?;
                    fence.lease_acquisitions.len() - 1
                }
            };
            if fence.lease_acquisitions[index].lease.is_none() {
                let lease = crate::autonomy::storage::acquire_placement_lease(
                    store,
                    &subject,
                    transaction,
                    "stado storage-root-reconcile",
                    LEASE_TTL_SECONDS,
                    Utc::now(),
                )
                .await
                .map_err(|error| DeployError(format!("cannot acquire {subject}: {error}")))?
                .ok_or_else(|| DeployError(format!("active placement lease blocks {subject}")))?;
                fence.lease_acquisitions[index].lease = Some(lease);
                fence.lease_acquisitions[index].status = "acquired".to_string();
                write_fence(storage_target, transaction, &fence, runner).await?;
            }
        }
        renew_fence_leases(store, &mut fence).await?;
        write_fence(storage_target, transaction, &fence, runner).await?;

        if !fence.queue.drained {
            if let Some(pause) = fence.queue.pause.as_ref() {
                if pause.status != "applied" {
                    if pause.status != "pause_intent" {
                        return Err(DeployError(format!(
                            "queue pause has invalid state {:?}",
                            pause.status
                        )));
                    }
                    match execute_queue_effect(store, pause).await? {
                        QueueEffectOutcome::Applied => {
                            fence
                                .queue
                                .pause
                                .as_mut()
                                .expect("queue pause was initialized")
                                .status = "applied".to_string();
                            write_fence(storage_target, transaction, &fence, runner).await?;
                        }
                        QueueEffectOutcome::Superseded(current) => {
                            fence
                                .queue
                                .pause
                                .as_mut()
                                .expect("queue pause was initialized")
                                .superseding = Some(current);
                            return Err(DeployError(
                                "queue control changed after the exact pause intent was recorded"
                                    .to_string(),
                            ));
                        }
                    }
                }
            }
            let current = crate::queue::control::read(store)
                .await
                .map_err(|error| DeployError(format!("cannot recheck queue fence: {error}")))?;
            if !current.paused {
                return Err(DeployError(
                    "queue is not paused after its durable fencing transition".to_string(),
                ));
            }
            let deadline = Instant::now()
                + Duration::from_secs(crate::queue::control::default_drain_timeout_s());
            while !crate::queue::control::is_drained(store)
                .await
                .map_err(|error| DeployError(format!("cannot prove queue drained: {error}")))?
            {
                if Instant::now() >= deadline {
                    return Err(DeployError(
                        "queue remained active until the canonical drain deadline; fence retained"
                            .to_string(),
                    ));
                }
                sleep(Duration::from_secs(5)).await;
            }
            fence.queue.drained = true;
            write_fence(storage_target, transaction, &fence, runner).await?;
        }
    }
    verify_resumable_writers(storage_target, transaction, &mut fence, runner).await?;
    fence_writers(
        storage_target,
        transaction,
        &mut fence,
        store.as_ref(),
        write_guard,
        runner,
    )
    .await?;
    fence.status = "fenced".to_string();
    fence.rechecked_at = Utc::now().timestamp();
    write_fence(storage_target, transaction, &fence, runner).await?;
    Ok(fence)
}
