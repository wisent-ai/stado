use super::*;

pub(in crate::deploy::host_storage_reconcile) async fn print_settled_label(
    target: &crate::targets::ComputeTarget,
    label: &str,
    runner: &Runner,
) -> Result<crate::deploy::service_label_print::LabelState, DeployError> {
    let mut state = crate::deploy::service_label_print::print_label(
        target,
        label,
        service::BootoutScope::Any,
        runner,
    )
    .await?;
    for _ in 0..2 {
        let complete = state.pid.is_none()
            || (state.process_started_at.is_some()
                && state.process_executable.is_some()
                && state.process_device.is_some()
                && state.process_inode.is_some()
                && state.process_sha256.is_some());
        if complete {
            return Ok(state);
        }
        sleep(Duration::from_secs(1)).await;
        state = crate::deploy::service_label_print::print_label(
            target,
            label,
            service::BootoutScope::Any,
            runner,
        )
        .await?;
    }
    Ok(state)
}

pub(in crate::deploy::host_storage_reconcile) async fn recheck_lifecycle_fence(
    storage_target: &crate::targets::ComputeTarget,
    transaction: &str,
    runner: &Runner,
) -> Result<LifecycleFence, DeployError> {
    let mut fence = read_fence(storage_target, transaction, runner)
        .await?
        .ok_or_else(|| DeployError("durable lifecycle fence is absent".to_string()))?;
    if fence.status != "fenced"
        || !fence.queue.drained
        || fence
            .writers
            .iter()
            .any(|writer| writer.status != "stopped")
    {
        return Err(DeployError(
            "durable lifecycle fence is not in the fenced/drained state".to_string(),
        ));
    }
    fence.resident_owner = resident_owner_retention(transaction)?;
    for writer in &fence.writers {
        let state = crate::deploy::service_label_print::print_label(
            storage_target,
            &writer.label,
            service::BootoutScope::Any,
            runner,
        )
        .await?;
        if state.loaded() || state.pid.is_some() {
            return Err(DeployError(format!(
                "writer {} on {} resumed during the storage fence",
                writer.label, writer.target
            )));
        }
        let autostart = service::label_autostart(storage_target, &writer.label, runner).await?;
        if writer
            .autostart
            .iter()
            .any(|(scope, enabled)| *enabled && autostart.get(scope) != Some(&false))
        {
            return Err(DeployError(format!(
                "writer {} became enabled during the storage fence",
                writer.label
            )));
        }
        if let Some(port) = writer.listener_port {
            prove_listener_closed(storage_target, port, runner).await?;
        }
    }
    for retained in &fence.transport_retained {
        let label = retained
            .get("label")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let state = crate::deploy::service_label_print::print_label(
            storage_target,
            label,
            service::BootoutScope::Any,
            runner,
        )
        .await?;
        if !state.loaded()
            || state.pid.is_none()
            || state.process_started_at.is_none()
            || state.process_device.is_none()
            || state.process_inode.is_none()
            || state.process_sha256.is_none()
        {
            return Err(DeployError(format!(
                "retained transport {label} is no longer a runnable mapped image"
            )));
        }
        let prior = retained
            .get("state")
            .and_then(Value::as_object)
            .ok_or_else(|| DeployError(format!("retained transport {label} has no prior state")))?;
        let current = state.to_json();
        for field in [
            "pid",
            "process_started_at",
            "process_executable",
            "process_device",
            "process_inode",
            "process_sha256",
        ] {
            if current.get(field) != prior.get(field) {
                return Err(DeployError(format!(
                    "retained transport {label} changed mapped identity field {field}"
                )));
            }
        }
        let autostart = service::label_autostart(storage_target, label, runner).await?;
        if retained.get("autostart") != Some(&json!(autostart)) {
            return Err(DeployError(format!(
                "retained transport {label} changed native autostart state"
            )));
        }
    }
    fence.rechecked_at = Utc::now().timestamp();
    write_fence(storage_target, transaction, &fence, runner).await?;
    Ok(fence)
}

pub(in crate::deploy::host_storage_reconcile) fn validate_prepared_fence(
    fence: &LifecycleFence,
) -> Result<(), DeployError> {
    if fence.roots.is_none() {
        return Err(DeployError(
            "lifecycle fence omitted its constructed storage roots".to_string(),
        ));
    }
    if fence.status != "preparing"
        && !fence.rollback_preparation
        && fence.preflight_evidence.is_none()
    {
        return Err(DeployError(
            "lifecycle fence omitted its frozen preflight evidence".to_string(),
        ));
    }
    let staged_runtime_digest = fence
        .staged_runtime
        .as_ref()
        .map(|release| release.staged_sha256.as_str())
        .filter(|digest| digest.len() == 64 && digest.bytes().all(|byte| byte.is_ascii_hexdigit()));
    for writer in &fence.writers {
        if let Some(snapshot) = &writer.unit_snapshot {
            let body = base64::engine::general_purpose::STANDARD
                .decode(&snapshot.body_base64)
                .map_err(|error| {
                    DeployError(format!(
                        "{} unit snapshot is invalid: {error}",
                        writer.label
                    ))
                })?;
            if hex::encode(Sha256::digest(&body)) != snapshot.sha256 {
                return Err(DeployError(format!(
                    "{} unit snapshot digest does not match its exact bytes",
                    writer.label
                )));
            }
        }
        if writer.was_runnable
            && (writer.prior_pid.is_none()
                || writer.prior_started_at.is_none()
                || writer.prior_executable.is_none()
                || writer.prior_device.is_none()
                || writer.prior_inode.is_none()
                || (writer.prior_sha256.is_none()
                    && (staged_runtime_digest.is_none()
                        || !writer
                            .prior_executable
                            .as_deref()
                            .is_some_and(|path| path.ends_with("/.stado/bin/stado")))))
        {
            return Err(DeployError(format!(
                "{} has no complete mapped-inode process identity or digest-verified canonical \
                 restoration",
                writer.label
            )));
        }
        for script in [
            writer.forward_object_recovery.as_ref(),
            writer.rollback_object_recovery.as_ref(),
        ]
        .into_iter()
        .flatten()
        {
            if hex::encode(Sha256::digest(script.body.as_bytes())) != script.sha256 {
                return Err(DeployError(format!(
                    "{} prepared recovery script digest changed",
                    writer.label
                )));
            }
        }
        if writer.role == "object-api"
            && (writer.forward_object_recovery.is_none()
                || writer.rollback_object_recovery.is_none())
        {
            return Err(DeployError(
                "object API has no immutable forward and captured-prior rollback configurations"
                    .to_string(),
            ));
        }
    }
    Ok(())
}
