use super::*;

#[allow(clippy::too_many_arguments)]
pub(super) async fn restore_fenced_writer(
    storage_target: &crate::targets::ComputeTarget,
    transaction: &str,
    fence: &mut LifecycleFence,
    index: usize,
    label: &str,
    rollback: bool,
    active_sha256: &str,
    roots: &StorageRoots,
    runner: &Runner,
) -> Result<(crate::deploy::service_label_print::LabelState, bool), DeployError> {
    let mut state = crate::deploy::service_label_print::print_label(
        storage_target,
        label,
        service::BootoutScope::Any,
        runner,
    )
    .await?;
    let mut autostart = service::label_autostart(storage_target, label, runner).await?;
    let unit_matches = if fence.writers[index].role == "object-api" {
        true
    } else {
        snapshot_unit_file(storage_target, &fence.writers[index].path, runner).await?
            == fence.writers[index].unit_snapshot
    };
    let was_durably_restored = fence.writers[index].status == "restored";
    let adopted = unit_matches
        && restored_state_matches(
            &fence.writers[index],
            &state,
            &autostart,
            active_sha256,
            roots,
            rollback,
        )
        && (!was_durably_restored || durable_restored_state_matches(&fence.writers[index], &state));
    if was_durably_restored && !adopted {
        return Err(DeployError(format!(
            "{label} drifted after its durable restored result"
        )));
    }
    if !adopted {
        if fence.writers[index].status != "restore_intent" {
            fence.writers[index].status = "restore_intent".to_string();
            write_fence(storage_target, transaction, fence, runner).await?;
        }
        if fence.writers[index].role != "object-api" && fence.writers[index].unit_snapshot.is_some()
        {
            restore_unit_snapshot(storage_target, &fence.writers[index], runner).await?;
        }
        let requires_load = fence.writers[index].was_loaded || fence.writers[index].was_runnable;
        if requires_load {
            if fence.writers[index].role == "object-api" {
                let prepared = if rollback {
                    fence.writers[index].rollback_object_recovery.as_ref()
                } else {
                    fence.writers[index].forward_object_recovery.as_ref()
                }
                .ok_or_else(|| {
                    DeployError(format!("{label} has no prepared recovery configuration"))
                })?;
                let recovered = host_channel::run_script_with_timeout(
                    storage_target,
                    &prepared.body,
                    Duration::from_secs(240),
                    runner,
                )
                .await?;
                if !recovered.ok() {
                    return Err(DeployError(format!(
                        "{label} did not restore through its prepared configuration: {}",
                        host_channel::last_error_line(&recovered, "remote command failed")
                    )));
                }
            } else {
                let writer = &fence.writers[index];
                if !writer.autostart.values().copied().any(|enabled| enabled) {
                    let scope = writer
                        .loaded_domains
                        .first()
                        .map(String::as_str)
                        .or_else(|| writer.autostart.keys().next().map(String::as_str))
                        .ok_or_else(|| {
                            DeployError(format!(
                                "{label} has no captured init-system scope for restoration"
                            ))
                        })?;
                    service::set_label_autostart(storage_target, label, scope, true, runner)
                        .await?;
                }
                let declared = managed_writer(storage_target, writer);
                let restarted = service::restart_service(storage_target, &declared, runner).await?;
                if !restarted.succeeded("restarted") {
                    return Err(DeployError(format!(
                        "{label} did not restore: {}",
                        restarted.failure()
                    )));
                }
            }
        }
        for (scope, enabled) in fence.writers[index].autostart.clone() {
            service::set_label_autostart(storage_target, label, &scope, enabled, runner).await?;
        }
        state = crate::deploy::service_label_print::print_label(
            storage_target,
            label,
            service::BootoutScope::Any,
            runner,
        )
        .await?;
        autostart = service::label_autostart(storage_target, label, runner).await?;
        if !restored_state_matches(
            &fence.writers[index],
            &state,
            &autostart,
            active_sha256,
            roots,
            rollback,
        ) {
            return Err(DeployError(format!(
                "{label} does not match its captured lifecycle and prepared runtime"
            )));
        }
        if fence.writers[index].role != "object-api"
            && snapshot_unit_file(storage_target, &fence.writers[index].path, runner).await?
                != fence.writers[index].unit_snapshot
        {
            return Err(DeployError(format!(
                "{label} unit definition differs from its captured exact bytes"
            )));
        }
    }
    Ok((state, was_durably_restored))
}
