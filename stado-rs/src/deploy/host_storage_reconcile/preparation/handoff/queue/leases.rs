use super::*;

pub(in crate::deploy::host_storage_reconcile) async fn renew_fence_leases(
    store: &crate::queue::JobStorage,
    fence: &mut LifecycleFence,
) -> Result<(), DeployError> {
    if fence
        .write_fence
        .as_ref()
        .is_some_and(|effect| matches!(effect.status.as_str(), "acquired" | "release_intent"))
    {
        return Ok(());
    }
    const LEASE_TTL_SECONDS: u64 = 12 * 60 * 60;
    for acquisition in &mut fence.lease_acquisitions {
        if matches!(
            acquisition.status.as_str(),
            "release_intent" | "released" | "superseded"
        ) {
            continue;
        }
        if acquisition.status != "acquired" {
            return Err(DeployError(format!(
                "placement lease {} has non-renewable state {:?}",
                acquisition.subject_id, acquisition.status
            )));
        }
        let lease = acquisition.lease.as_mut().ok_or_else(|| {
            DeployError(format!(
                "placement lease acquisition for {} has no durable result",
                acquisition.subject_id
            ))
        })?;
        let renewed = crate::autonomy::storage::renew_placement_lease(
            store,
            &lease.subject_id,
            &lease.token,
            LEASE_TTL_SECONDS,
            Utc::now(),
        )
        .await
        .map_err(|error| DeployError(format!("cannot renew {}: {error}", lease.subject_id)))?;
        *lease = match renewed {
            Some(renewed) => renewed,
            None => crate::autonomy::storage::acquire_placement_lease(
                store,
                &lease.subject_id,
                &fence.transaction,
                &lease.holder,
                LEASE_TTL_SECONDS,
                Utc::now(),
            )
            .await
            .map_err(|error| {
                DeployError(format!(
                    "cannot recover lease {}: {error}",
                    lease.subject_id
                ))
            })?
            .ok_or_else(|| {
                DeployError(format!(
                    "placement lease ownership changed for {}",
                    lease.subject_id
                ))
            })?,
        };
    }
    Ok(())
}

pub(in crate::deploy::host_storage_reconcile) async fn release_fence_leases(
    storage_target: &crate::targets::ComputeTarget,
    transaction: &str,
    store: &crate::queue::JobStorage,
    fence: &mut LifecycleFence,
    runner: &Runner,
) -> Result<(), DeployError> {
    for index in 0..fence.lease_acquisitions.len() {
        let status = fence.lease_acquisitions[index].status.as_str();
        if matches!(status, "released" | "superseded") {
            continue;
        }
        if status == "acquired" {
            let mut released = fence.lease_acquisitions[index]
                .lease
                .clone()
                .ok_or_else(|| {
                    DeployError(format!(
                        "placement lease acquisition for {} has no durable result",
                        fence.lease_acquisitions[index].subject_id
                    ))
                })?;
            released.expires_at = Utc::now().to_rfc3339();
            fence.lease_acquisitions[index].released_lease = Some(released);
            fence.lease_acquisitions[index].status = "release_intent".to_string();
            write_fence(storage_target, transaction, fence, runner).await?;
        } else if status != "release_intent" {
            return Err(DeployError(format!(
                "placement lease {} has non-releasable state {:?}",
                fence.lease_acquisitions[index].subject_id, status
            )));
        }
        let acquisition = &fence.lease_acquisitions[index];
        let owned = acquisition.lease.as_ref().ok_or_else(|| {
            DeployError(format!(
                "placement lease acquisition for {} has no durable result",
                acquisition.subject_id
            ))
        })?;
        let released = acquisition.released_lease.as_ref().ok_or_else(|| {
            DeployError(format!(
                "placement lease release for {} has no durable intended result",
                acquisition.subject_id
            ))
        })?;
        let relinquished =
            crate::autonomy::storage::release_placement_lease_exact(store, owned, released)
                .await
                .map_err(|error| {
                    DeployError(format!(
                        "cannot release placement lease {}: {error}",
                        acquisition.subject_id
                    ))
                })?;
        fence.lease_acquisitions[index].status = if relinquished {
            "released"
        } else {
            "superseded"
        }
        .to_string();
        write_fence(storage_target, transaction, fence, runner).await?;
    }
    Ok(())
}
