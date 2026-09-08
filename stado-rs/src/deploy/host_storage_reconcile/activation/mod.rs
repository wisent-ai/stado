use super::*;

mod restore;
mod rollback;
mod route;
mod writer;

pub(in crate::deploy::host_storage_reconcile) use rollback::*;

use restore::*;
use route::*;
use writer::*;

pub(super) async fn activate_lifecycle_fence(
    storage_target: &crate::targets::ComputeTarget,
    transaction: &str,
    runner: &Runner,
    rollback: bool,
    write_guard: &mut Option<std::fs::File>,
) -> Result<LifecycleFence, DeployError> {
    let mut fence = read_fence(storage_target, transaction, runner)
        .await?
        .ok_or_else(|| DeployError("durable lifecycle fence is absent".to_string()))?;
    validate_prepared_fence(&fence)?;
    refresh_resident_owner(storage_target, transaction, &mut fence, runner).await?;
    let preflight = if fence.rollback_preparation {
        None
    } else {
        Some(read_json_evidence(
            transaction,
            PREFLIGHT_EVIDENCE_FILE,
            fence.preflight_evidence.as_ref().ok_or_else(|| {
                DeployError("lifecycle fence omitted frozen preflight evidence".to_string())
            })?,
            "preflight evidence",
        )?)
    };
    let roots = fence.roots.clone().ok_or_else(|| {
        DeployError("lifecycle fence omitted its observed storage roots".to_string())
    })?;
    let route_conflict_winner = if roots.prior_primary == roots.primary {
        "primary"
    } else {
        "backup"
    };
    let conflict_winner = if rollback {
        route_conflict_winner.to_string()
    } else {
        let receipt = read_transaction_receipt(transaction)?;
        let pinned = receipt
            .get("conflict_winner")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                DeployError("checkpoint receipt omitted its conflict winner".to_string())
            })?;
        if pinned != route_conflict_winner {
            return Err(DeployError(
                "checkpoint conflict winner differs from the captured storage route".to_string(),
            ));
        }
        pinned.to_string()
    };
    let final_status = if rollback { "rolled_back" } else { "activated" };
    let admissible = if rollback {
        matches!(
            fence.status.as_str(),
            "preparing" | "fenced" | "rolling_back" | "restoring" | "rolled_back"
        )
    } else {
        matches!(
            fence.status.as_str(),
            "fenced" | "activating" | "restoring" | "activated"
        )
    };
    if !admissible {
        return Err(DeployError(format!(
            "lifecycle fence cannot {} from {}",
            if rollback { "roll back" } else { "activate" },
            fence.status
        )));
    }
    if fence.status == final_status {
        return Ok(fence);
    }
    if !matches!(fence.status.as_str(), "activating" | "restoring") {
        fence.status = if rollback {
            "rolling_back"
        } else {
            "activating"
        }
        .to_string();
        write_fence(storage_target, transaction, &fence, runner).await?;
    }

    if fence.write_fence.is_some() {
        acquire_storage_write_fence(storage_target, transaction, &mut fence, write_guard, runner)
            .await?;
    }
    let staged_runtime = fence
        .staged_runtime
        .clone()
        .ok_or_else(|| DeployError("lifecycle fence has no staged declared runtime".to_string()))?;
    let active_sha256 = crate::deploy::host_release::activate_staged_program(
        storage_target,
        &staged_runtime,
        runner,
    )
    .await?;
    if fence
        .activation_sha256
        .as_deref()
        .is_some_and(|digest| digest != active_sha256)
    {
        return Err(DeployError(
            "persisted activation digest differs from the adopted active runtime".to_string(),
        ));
    }
    fence.activation_sha256 = Some(active_sha256.clone());
    fence
        .activated_at
        .get_or_insert_with(|| Utc::now().timestamp());
    fence.status = "restoring".to_string();
    write_fence(storage_target, transaction, &fence, runner).await?;

    let mut order = (0..fence.writers.len()).collect::<Vec<_>>();
    order.sort_by_key(|index| restore_priority(&fence.writers[*index].role));
    let mut restored_store = None;
    for index in order {
        let label = fence.writers[index].label.clone();
        let (state, was_durably_restored) = restore_fenced_writer(
            storage_target,
            transaction,
            &mut fence,
            index,
            &label,
            rollback,
            &active_sha256,
            &roots,
            runner,
        )
        .await?;
        let restored_route = restored_object_route(
            storage_target,
            &fence,
            index,
            &label,
            &state,
            was_durably_restored,
            rollback,
            &roots,
            preflight.as_ref(),
            &conflict_winner,
            runner,
        )
        .await?;
        fence.writers[index].restored_pid = state.pid;
        fence.writers[index].restored_started_at = state.process_started_at;
        fence.writers[index].restored_loaded_environment = state.loaded_environment;
        fence.writers[index].restored_executable = state.process_executable;
        fence.writers[index].restored_sha256 = state.process_sha256;
        fence.writers[index].restored_device = state.process_device;
        fence.writers[index].restored_inode = state.process_inode;
        fence.writers[index].restored_route = restored_route;
        fence.writers[index].status = "restored".to_string();
        write_fence(storage_target, transaction, &fence, runner).await?;
        if fence.writers[index].role == "object-api" {
            release_storage_write_fence(
                storage_target,
                transaction,
                &mut fence,
                write_guard,
                runner,
            )
            .await?;
            let store = recovered_object_store(&fence)?;
            renew_fence_leases(&store, &mut fence).await?;
            write_fence(storage_target, transaction, &fence, runner).await?;
            restored_store = Some(store);
        } else if restored_store.is_none() {
            return Err(DeployError(
                "a writer would resume before the object API restored A and renewed every lease"
                    .to_string(),
            ));
        }
    }

    let store = restored_store.ok_or_else(|| {
        DeployError("object API did not establish the recovered authority queue".to_string())
    })?;
    release_fence_leases(storage_target, transaction, &store, &mut fence, runner).await?;
    restore_queue_control(
        storage_target,
        transaction,
        &store,
        &mut fence,
        rollback,
        runner,
    )
    .await?;
    fence.status = final_status.to_string();
    fence.restored_at = Some(Utc::now().timestamp());
    write_fence(storage_target, transaction, &fence, runner).await?;
    Ok(fence)
}
