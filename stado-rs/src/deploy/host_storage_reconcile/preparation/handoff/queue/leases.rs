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
    // The transaction holds its leases until it releases them; renewing
    // records the process now carrying the transaction, so a resume after a
    // crash adopts them and nothing expires under a running handoff.
    for acquisition in &mut fence.lease_acquisitions {
        if matches!(
            acquisition.status.as_str(),
            "release_intent" | "released" | "superseded"
        ) {
            continue;
        }
        if acquisition.status != "acquired" {
            return Err(DeployError::unreachable(format!(
                "placement lease {} has non-renewable state {:?}",
                acquisition.subject_id, acquisition.status
            )));
        }
        let lease = acquisition.lease.as_mut().ok_or_else(|| {
            DeployError(format!(
                "placement lease acquisition for {} has no durable result",
                acquisition.subject_id
            ))
            .stating(crate::primitives::failure::FailureCode::InfraDown)
        })?;
        let renewed = crate::autonomy::storage::renew_placement_lease(
            store,
            &lease.subject_id,
            &lease.token,
            None,
            Utc::now(),
        )
        .await
        .map_err(|error| {
            DeployError::from(error).within(format!("cannot renew {}", lease.subject_id))
        })?;
        *lease = match renewed {
            Some(renewed) => renewed,
            None => crate::autonomy::storage::acquire_placement_lease(
                store,
                &lease.subject_id,
                &fence.transaction,
                &lease.holder,
                None,
                Utc::now(),
            )
            .await
            .map_err(|error| {
                DeployError::from(error).within(format!("cannot recover lease {}", lease.subject_id))
            })?
            .ok_or_else(|| {
                DeployError(format!(
                    "placement lease ownership changed for {}",
                    lease.subject_id
                ))
                .stating(crate::primitives::failure::FailureCode::Refused)
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
                    .stating(crate::primitives::failure::FailureCode::InfraDown)
                })?;
            released.expires_at = Utc::now().to_rfc3339();
            fence.lease_acquisitions[index].released_lease = Some(released);
            fence.lease_acquisitions[index].status = "release_intent".to_string();
            write_fence(storage_target, transaction, fence, runner).await?;
        } else if status != "release_intent" {
            return Err(DeployError(format!(
                "placement lease {} has non-releasable state {:?}",
                fence.lease_acquisitions[index].subject_id, status
            ))
            .stating(crate::primitives::failure::FailureCode::InfraDown));
        }
        let acquisition = &fence.lease_acquisitions[index];
        let owned = acquisition.lease.as_ref().ok_or_else(|| {
            DeployError(format!(
                "placement lease acquisition for {} has no durable result",
                acquisition.subject_id
            ))
            .stating(crate::primitives::failure::FailureCode::InfraDown)
        })?;
        let released = acquisition.released_lease.as_ref().ok_or_else(|| {
            DeployError(format!(
                "placement lease release for {} has no durable intended result",
                acquisition.subject_id
            ))
            .stating(crate::primitives::failure::FailureCode::InfraDown)
        })?;
        let relinquished =
            crate::autonomy::storage::release_placement_lease_exact(store, owned, released)
                .await
                .map_err(|error| {
                    DeployError::from(error).within(format!(
                        "cannot release placement lease {}",
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
