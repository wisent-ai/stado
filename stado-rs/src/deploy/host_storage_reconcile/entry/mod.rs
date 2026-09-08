use super::*;

mod host;
mod launch;
mod owner;
mod worker;

pub use host::reconcile_host;
pub use worker::reconcile_host_worker;

use launch::*;
use owner::*;

async fn reconcile_host_inner(
    target: &crate::targets::ComputeTarget,
    transaction: &str,
    phase: &str,
    runner: &Runner,
) -> Result<Value, DeployError> {
    validate_transaction(transaction)?;
    if !matches!(phase, RUN | RESUME | STATUS | ROLLBACK | FINALIZE) {
        return Err(DeployError(format!(
            "phase must be {RUN}, {RESUME}, {STATUS}, {ROLLBACK}, or {FINALIZE}, not {phase:?}"
        )));
    }
    if phase == STATUS {
        let receipt = remote_phase(target, transaction, STATUS, runner).await?;
        let fence = read_fence(target, transaction, runner).await?;
        return report(target, transaction, phase, receipt, fence.as_ref());
    }
    let mut write_guard = None;
    let existing = read_fence(target, transaction, runner).await?;
    // The captured target keeps the host reachable across the outage. Its
    // managed version is not a release declaration: a remote caller may have
    // captured an older registry. Before fencing, resolve the resident host's
    // authoritative declaration; afterwards, keep the staged coordinate pinned.
    let runtime_version = match existing.as_ref() {
        Some(fence) => {
            if fence.schema != FENCE_SCHEMA || fence.transaction != transaction {
                return Err(DeployError(
                    "durable lifecycle fence belongs to another transaction".to_string(),
                ));
            }
            Some(
                fence
                    .staged_runtime
                    .as_ref()
                    .ok_or_else(|| {
                        DeployError(
                            "durable lifecycle fence omitted its staged runtime".to_string(),
                        )
                    })?
                    .request
                    .version
                    .clone(),
            )
        }
        None if matches!(phase, RUN | RESUME) => {
            let registry = crate::targets::fetch_registry_remote()
                .await
                .map_err(|error| DeployError(error.to_string()))?;
            let declared = host_channel::resolve_target(&registry, &target.name)?;
            Some(
                declared
                    .declared_version("stado")
                    .ok_or_else(|| {
                        DeployError("storage host has no declared Stado runtime".to_string())
                    })?
                    .to_string(),
            )
        }
        None => None,
    };
    let mut runtime_target = target.clone();
    if let Some(version) = runtime_version {
        runtime_target
            .managed_versions
            .insert("stado".to_string(), version);
    }
    let target = &runtime_target;
    if phase == FINALIZE {
        let mut fence =
            existing.ok_or_else(|| DeployError("durable lifecycle fence is absent".to_string()))?;
        refresh_resident_owner(target, transaction, &mut fence, runner).await?;
        if fence.status != "activated" {
            return Err(DeployError(format!(
                "finalize observes lifecycle cleanup only after activation, not {}",
                fence.status
            )));
        }
        let observations = typed_final_lifecycle_observations(transaction, &fence).await?;
        let receipt =
            record_typed_final_lifecycle_observations(target, transaction, &observations, runner)
                .await?;
        return report(target, transaction, phase, receipt, Some(&fence));
    }
    if phase == ROLLBACK
        || existing
            .as_ref()
            .is_some_and(|fence| fence.rollback_preparation)
    {
        let receipt = remote_phase(target, transaction, STATUS, runner).await?;
        let receipt_status = receipt
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let mut rollback_fence = existing
            .ok_or_else(|| DeployError("rollback has no recorded lifecycle fence".to_string()))?;
        if receipt_status == "absent" {
            if !rollback_fence.rollback_preparation
                && (rollback_fence.status != "preparing"
                    || !rollback_fence.queue.drained
                    || rollback_fence.lease_acquisitions.len() != rollback_fence.writers.len()
                    || rollback_fence
                        .lease_acquisitions
                        .iter()
                        .any(|entry| entry.status != "acquired" || entry.lease.is_none()))
            {
                return Err(DeployError(
                    "preparation rollback requires the recorded drained queue and complete \
                     placement leases"
                        .to_string(),
                ));
            }
            rollback_fence.rollback_preparation = true;
            write_fence(target, transaction, &rollback_fence, runner).await?;
            let fence =
                activate_lifecycle_fence(target, transaction, runner, true, &mut write_guard)
                    .await?;
            return report(
                target,
                transaction,
                phase,
                json!({
                    "schema": "stado.storage-root-reconcile.v2",
                    "transaction": transaction,
                    "status": "preparation_rolled_back",
                    "data_mutated": false,
                }),
                Some(&fence),
            );
        }
        if !matches!(
            receipt_status,
            "checkpoint_ready" | "applying" | "rollback_effects_armed"
        ) {
            return Err(DeployError(format!(
                "rollback is only safe before data activation, not receipt state {receipt_status:?}"
            )));
        }
        verify_resident_lock(transaction)?;
        acquire_storage_write_fence(
            target,
            transaction,
            &mut rollback_fence,
            &mut write_guard,
            runner,
        )
        .await?;
        let receipt = remote_phase(target, transaction, ARM_ROLLBACK, runner).await?;
        let fence =
            activate_lifecycle_fence(target, transaction, runner, true, &mut write_guard).await?;
        return report(target, transaction, phase, receipt, Some(&fence));
    }

    if existing
        .as_ref()
        .is_some_and(|fence| fence.status == "rolled_back")
    {
        return Err(DeployError(
            "a rolled-back transaction cannot be reactivated; choose a new transaction id"
                .to_string(),
        ));
    }
    let mut fence = match existing {
        Some(fence)
            if matches!(
                fence.status.as_str(),
                "activating" | "restoring" | "activated"
            ) =>
        {
            fence
        }
        _ => prepare_lifecycle_fence(target, transaction, runner, &mut write_guard).await?,
    };
    if fence.status == "fenced" {
        remote_phase(target, transaction, CHECKPOINT, runner).await?;
        let checkpoint_decisions = typed_lifecycle_decisions(transaction).await?;
        record_typed_lifecycle_decisions(target, transaction, &checkpoint_decisions, runner)
            .await?;
        fence = recheck_lifecycle_fence(target, transaction, runner).await?;
        let receipt_status = read_transaction_receipt(transaction)?
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        if receipt_status != "activation_effects_armed" {
            if !matches!(
                receipt_status.as_str(),
                "checkpoint_ready" | "applying" | "data_committed_pending_activation"
            ) {
                return Err(DeployError(format!(
                    "fenced transaction has non-resumable receipt state {receipt_status:?}"
                )));
            }
            remote_phase(target, transaction, APPLY, runner).await?;
            let committed_decisions = typed_lifecycle_decisions(transaction).await?;
            if committed_decisions != checkpoint_decisions {
                return Err(DeployError(
                    "typed lifecycle decisions changed between checkpoint and data commit"
                        .to_string(),
                ));
            }
            record_typed_lifecycle_decisions(target, transaction, &committed_decisions, runner)
                .await?;
            fence = recheck_lifecycle_fence(target, transaction, runner).await?;
            remote_phase(target, transaction, ARM_ACTIVATION, runner).await?;
        }
        if read_transaction_receipt(transaction)?
            .get("status")
            .and_then(Value::as_str)
            != Some("activation_effects_armed")
        {
            return Err(DeployError(
                "activation-effect boundary was not durably recorded".to_string(),
            ));
        }
        verify_resident_lock(transaction)?;
        validate_prepared_fence(&fence)?;
        fence =
            activate_lifecycle_fence(target, transaction, runner, false, &mut write_guard).await?;
    } else if fence.status != "activated" {
        let receipt = read_transaction_receipt(transaction)?;
        if receipt.get("status").and_then(Value::as_str) != Some("activation_effects_armed") {
            return Err(DeployError(
                "partial activation has no durable activation-effect boundary".to_string(),
            ));
        }
        let decisions = typed_lifecycle_decisions(transaction).await?;
        record_typed_lifecycle_decisions(target, transaction, &decisions, runner).await?;
        verify_resident_lock(transaction)?;
        validate_prepared_fence(&fence)?;
        fence =
            activate_lifecycle_fence(target, transaction, runner, false, &mut write_guard).await?;
    }
    let receipt = remote_phase(target, transaction, ACTIVATE, runner).await?;
    report(target, transaction, phase, receipt, Some(&fence))
}
