use super::*;

pub(super) async fn verify_resumable_writers(
    storage_target: &crate::targets::ComputeTarget,
    transaction: &str,
    fence: &mut LifecycleFence,
    runner: &Runner,
) -> Result<(), DeployError> {
    for index in 0..fence.writers.len() {
        let writer = &fence.writers[index];
        let current = crate::deploy::service_label_print::print_label(
            storage_target,
            &writer.label,
            service::BootoutScope::Any,
            runner,
        )
        .await?;
        let current_autostart =
            service::label_autostart(storage_target, &writer.label, runner).await?;
        let process_matches_prior = current.loaded() == writer.was_loaded
            && current.pid.as_deref() == writer.prior_pid.as_deref()
            && current.process_started_at.as_deref() == writer.prior_started_at.as_deref()
            && current.process_executable.as_deref() == writer.prior_executable.as_deref()
            && current.process_device == writer.prior_device
            && current.process_inode == writer.prior_inode
            && match writer.prior_sha256.as_deref() {
                Some(expected) => current.process_sha256.as_deref() == Some(expected),
                None => writer
                    .prior_executable
                    .as_deref()
                    .is_some_and(|path| path.ends_with("/.stado/bin/stado")),
            }
            && current.loaded_environment == writer.prior_loaded_environment;
        match writer.status.as_str() {
            "pending" if process_matches_prior && current_autostart == writer.autostart => {}
            "stop_intent" if !current.loaded() && current.pid.is_none() => {
                if writer.autostart.iter().any(|(scope, enabled)| {
                    *enabled && current_autostart.get(scope) != Some(&false)
                }) {
                    return Err(DeployError(format!(
                        "{} stopped after an interrupted fence but remained enabled",
                        writer.label
                    )));
                }
                fence.writers[index].status = "stopped".to_string();
                write_fence(storage_target, transaction, fence, runner).await?;
            }
            "stop_intent"
                if process_matches_prior
                    && writer.autostart.iter().all(|(scope, prior)| {
                        current_autostart
                            .get(scope)
                            .is_some_and(|current| current == prior || (*prior && !*current))
                    }) => {}
            "stopped" if !current.loaded() && current.pid.is_none() => {}
            state => {
                return Err(DeployError(format!(
                    "{} native state does not match resumable fence state {state:?}",
                    writer.label
                )));
            }
        }
    }
    Ok(())
}

pub(super) async fn fence_writers(
    storage_target: &crate::targets::ComputeTarget,
    transaction: &str,
    fence: &mut LifecycleFence,
    store: Option<&crate::queue::JobStorage>,
    write_guard: &mut Option<std::fs::File>,
    runner: &Runner,
) -> Result<(), DeployError> {
    for index in 0..fence.writers.len() {
        if fence.writers[index].status == "stopped" {
            continue;
        }
        if fence.writers[index].role == "object-api" {
            acquire_storage_write_fence(storage_target, transaction, fence, write_guard, runner)
                .await?;
            capture_fenced_preflight(storage_target, transaction, fence, runner).await?;
        }
        if fence.writers[index].status == "pending" {
            fence.writers[index].status = "stop_intent".to_string();
            write_fence(storage_target, transaction, fence, runner).await?;
        }
        if let Some(store) = store {
            renew_fence_leases(store, fence).await?;
        }
        write_fence(storage_target, transaction, fence, runner).await?;
        let label = fence.writers[index].label.clone();
        for (scope, enabled) in fence.writers[index].autostart.clone() {
            if enabled {
                service::set_label_autostart(storage_target, &label, &scope, false, runner).await?;
            }
        }
        let disabled = service::label_autostart(storage_target, &label, runner).await?;
        if fence.writers[index]
            .autostart
            .iter()
            .any(|(scope, enabled)| *enabled && disabled.get(scope) != Some(&false))
        {
            return Err(DeployError(format!(
                "{label} remained enabled after persistent lifecycle disable"
            )));
        }
        if fence.writers[index].was_loaded || fence.writers[index].was_runnable {
            let (state, detail) =
                service::bootout_label(storage_target, &label, service::BootoutScope::Any, runner)
                    .await?;
            if !matches!(state.as_str(), "booted_out" | "absent") {
                return Err(DeployError(format!("{label} did not boot out: {detail}")));
            }
        }
        let state = crate::deploy::service_label_print::print_label(
            storage_target,
            &label,
            service::BootoutScope::Any,
            runner,
        )
        .await?;
        if state.loaded() || state.pid.is_some() {
            return Err(DeployError(format!(
                "{label} remained loaded after writer fencing"
            )));
        }
        if let Some(port) = fence.writers[index].listener_port {
            prove_listener_closed(storage_target, port, runner).await?;
        }
        fence.writers[index].status = "stopped".to_string();
        write_fence(storage_target, transaction, fence, runner).await?;
    }
    Ok(())
}
