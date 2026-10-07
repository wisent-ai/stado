use super::*;

pub(super) async fn restore_unit_snapshot(
    target: &crate::targets::ComputeTarget,
    writer: &WriterFence,
    runner: &Runner,
) -> Result<(), DeployError> {
    let snapshot = writer.unit_snapshot.as_ref().ok_or_else(|| {
        DeployError(format!("{} has no captured exact unit bytes", writer.label))
            .stating(crate::primitives::failure::FailureCode::InfraDown)
    })?;
    let command = host_step_script(&[
        "restore-unit",
        "--path",
        &writer.path,
        "--sha256",
        &snapshot.sha256,
        "--mode",
        &snapshot.mode.to_string(),
        "--uid",
        &snapshot.uid.to_string(),
        "--gid",
        &snapshot.gid.to_string(),
    ])?;
    let script = format!(
        "STADO_UNIT_BODY={} {command}",
        shlex_quote(&snapshot.body_base64)
    );
    let output = host_channel::run_script(target, &script, runner).await?;
    let marker = format!("STADO_UNIT_RESTORED\t{}", snapshot.sha256);
    if !output.ok() || !output.stdout.lines().any(|line| line == marker) {
        return Err(DeployError::unreachable(format!(
            "exact unit restoration failed for {} on {}: {}",
            writer.label,
            target.name,
            remote_failure_detail(&output, "remote command failed")
        )));
    }
    Ok(())
}

pub(super) fn restored_state_matches(
    writer: &WriterFence,
    state: &crate::deploy::service_label_print::LabelState,
    autostart: &BTreeMap<String, bool>,
    active_sha256: &str,
    roots: &StorageRoots,
    rollback: bool,
) -> bool {
    if autostart != &writer.autostart {
        return false;
    }
    let should_be_loaded = writer.was_loaded || writer.was_runnable;
    if state.loaded() != should_be_loaded {
        return false;
    }
    if !should_be_loaded {
        return state.pid.is_none();
    }
    if let Some(pid) = state.pid.as_deref() {
        if pid == "0"
            || state.process_started_at.is_none()
            || state.process_executable.is_none()
            || state.process_device.is_none()
            || state.process_inode.is_none_or(|inode| inode == 0)
        {
            return false;
        }
        let expected_sha256 = if writer.role == "object-api"
            || writer
                .prior_executable
                .as_deref()
                .is_some_and(|path| executable_name(path) == "stado")
        {
            Some(active_sha256)
        } else {
            writer.prior_sha256.as_deref()
        };
        if state.process_sha256.as_deref() != expected_sha256 {
            return false;
        }
    } else if writer.role == "object-api"
        || (state.state.is_none()
            && state.last_exit_code.is_none()
            && state.restart.is_none()
            && state.triggers.is_none())
    {
        return false;
    }
    if writer.role != "object-api" {
        return true;
    }
    let loaded = &state.loaded_environment;
    if !loaded.contains_key("WC_STORAGE_BACKEND") {
        return true;
    }
    let expected_config = writer
        .prior_loaded_environment
        .get("STADO_CONFIG")
        .or_else(|| writer.unit_declared_environment.get("STADO_CONFIG"))
        .or_else(|| writer.registry_declared_environment.get("STADO_CONFIG"))
        .map(String::as_str);
    if loaded.get("WC_STORAGE_BACKEND").map(String::as_str) != Some("local")
        || loaded.get("STADO_CONFIG").map(String::as_str) != expected_config
    {
        return false;
    }
    let (primary, backup) = if rollback {
        (roots.prior_primary.as_str(), roots.prior_backup.as_deref())
    } else {
        (roots.primary.as_str(), Some(roots.backup.as_str()))
    };
    loaded.get("WC_LOCAL_STORAGE_PATH").map(String::as_str) == Some(primary)
        && loaded
            .get("WC_BACKUP_STORAGE_BACKEND")
            .map(String::as_str)
            .filter(|value| !value.is_empty())
            == backup.map(|_| "local")
        && loaded
            .get("WC_BACKUP_LOCAL_STORAGE_PATH")
            .map(String::as_str)
            .filter(|value| !value.is_empty())
            == backup
}

pub(super) fn durable_restored_state_matches(
    writer: &WriterFence,
    state: &crate::deploy::service_label_print::LabelState,
) -> bool {
    (writer.role != "object-api" || writer.restored_route.is_some())
        && state.pid == writer.restored_pid
        && state.process_started_at == writer.restored_started_at
        && state.loaded_environment == writer.restored_loaded_environment
        && state.process_executable == writer.restored_executable
        && state.process_sha256 == writer.restored_sha256
        && state.process_device == writer.restored_device
        && state.process_inode == writer.restored_inode
}
