use super::*;

#[allow(clippy::too_many_arguments)]
pub(super) async fn fenced_writer(
    candidate: &ServiceCandidate,
    current_runner: Option<&str>,
    staged_runtime: &crate::deploy::host_release::StagedRelease,
    object_port: &mut Option<u16>,
    owning_runner_found: &mut bool,
    transport_retained: &mut Vec<Value>,
    non_storage_retained: &mut Vec<Value>,
    runner: &Runner,
) -> Result<Option<WriterFence>, DeployError> {
    let observed_role = service_role(candidate.declared.unit_id(), &candidate.observed_command);
    if observed_role == "other" && candidate.storage_evidence.is_empty() {
        non_storage_retained.push(json!({
            "target": candidate.target.name.clone(),
            "label": candidate.declared.unit_id(),
            "loaded_domains": candidate.loaded_domains.clone(),
            "observed_command": candidate.observed_command.clone(),
            "reason": "no Stado/runner/object-API role or local-storage route evidence",
        }));
        return Ok(None);
    }
    let state =
        print_settled_label(&candidate.target, candidate.declared.unit_id(), runner).await?;
    let command = state.runs().unwrap_or(&candidate.observed_command);
    let mut role = service_role(candidate.declared.unit_id(), command).to_string();
    if role == "other" {
        role = "writer".to_string();
    }
    if role == "runner"
        && current_runner
            .is_some_and(|current| current_runner_candidate(candidate, command, current))
    {
        role = "current-runner".to_string();
        *owning_runner_found = true;
    }
    let autostart =
        service::label_autostart(&candidate.target, candidate.declared.unit_id(), runner).await?;
    if role == "object-api" {
        let backup_backend = state.loaded_environment.get("WC_BACKUP_STORAGE_BACKEND");
        let backup_path = state.loaded_environment.get("WC_BACKUP_LOCAL_STORAGE_PATH");
        if state.pid.is_none()
            || state.process_started_at.is_none()
            || state.process_executable.is_none()
            || state.process_device.is_none()
            || state.process_inode.is_none()
            || state.process_sha256.is_none()
        {
            return Err(DeployError(format!(
                "{} cannot be fenced without a mapped-inode image identity",
                candidate.declared.unit_id()
            )));
        }
        let loaded_routing_observed = state
            .loaded_environment
            .get("WC_STORAGE_BACKEND")
            .map(String::as_str)
            == Some("local")
            && state
                .loaded_environment
                .get("WC_LOCAL_STORAGE_PATH")
                .is_some_and(|path| !path.is_empty())
            && state
                .loaded_environment
                .get("STADO_CONFIG")
                .is_some_and(|path| !path.is_empty())
            && backup_backend.is_some() == backup_path.is_some();
        if state.loaded_environment.contains_key("WC_STORAGE_BACKEND") && !loaded_routing_observed {
            return Err(DeployError(format!(
                "{} reported an incomplete loaded storage route",
                candidate.declared.unit_id()
            )));
        }
        *object_port = command_u16_option(command, "--port");
    }
    if matches!(role.as_str(), "transport" | "current-runner") {
        if role == "current-runner" && state.pid.is_none() {
            return Err(DeployError(
                "Actions runner gate did not map its owning live native process".to_string(),
            ));
        }
        if state.pid.is_some()
            && (state.process_started_at.is_none()
                || state.process_device.is_none()
                || state.process_inode.is_none()
                || state.process_sha256.is_none())
        {
            return Err(DeployError(format!(
                "retained transport {} has no mapped-inode image identity",
                candidate.declared.unit_id()
            )));
        }
        if state.loaded() || state.pid.is_some() {
            transport_retained.push(json!({
                "host": candidate.target.name.clone(),
                "label": candidate.declared.unit_id(),
                "loaded_domains": candidate.loaded_domains.clone(),
                "autostart": autostart,
                "state": state.to_json(),
            }));
        }
        return Ok(None);
    }
    let was_loaded = state.loaded() || !candidate.loaded_domains.is_empty();
    let was_runnable = state.pid.is_some();
    let canonical_stado_recovery = state
        .process_executable
        .as_deref()
        .is_some_and(|path| path.ends_with("/.stado/bin/stado"))
        && staged_runtime.staged_sha256.len() == 64;
    if was_runnable
        && (state.process_started_at.is_none()
            || state.process_executable.is_none()
            || state.process_device.is_none()
            || state.process_inode.is_none()
            || (state.process_sha256.is_none() && !canonical_stado_recovery))
    {
        return Err(DeployError(format!(
            "{} cannot be fenced without a mapped-inode process identity or its \
             digest-verified canonical restoration plan",
            candidate.declared.unit_id()
        )));
    }
    if (was_loaded || was_runnable) && candidate.declared.path.is_empty() {
        return Err(DeployError(format!(
            "{} has no unit path from which its exact prior lifecycle can be restored",
            candidate.declared.unit_id()
        )));
    }
    let listener_port = (role == "object-api").then_some(*object_port).flatten();
    if role == "object-api" && listener_port.is_none() {
        return Err(DeployError(
            "object API listener port is absent from its loaded argv".to_string(),
        ));
    }
    let pending = was_loaded || was_runnable || autostart.values().copied().any(|enabled| enabled);
    let unit_snapshot =
        snapshot_unit_file(&candidate.target, &candidate.declared.path, runner).await?;
    if pending && unit_snapshot.is_none() {
        return Err(DeployError(format!(
            "{} has no exact unit bytes for restoration",
            candidate.declared.unit_id()
        )));
    }
    let unit_declared_environment = unit_declared_environment(candidate, unit_snapshot.as_ref())?;
    Ok(Some(WriterFence {
        target: candidate.target.name.clone(),
        label: candidate.declared.unit_id().to_string(),
        role,
        storage_evidence: candidate.storage_evidence.iter().cloned().collect(),
        path: candidate.declared.path.clone(),
        listener_port,
        was_loaded,
        was_runnable,
        loaded_domains: candidate.loaded_domains.clone(),
        autostart,
        prior_pid: state.pid,
        prior_started_at: state.process_started_at,
        prior_loaded_environment: state.loaded_environment,
        registry_declared_environment: candidate.declared.env.clone(),
        unit_declared_environment,
        prior_executable: state.process_executable,
        prior_sha256: state.process_sha256,
        prior_device: state.process_device,
        prior_inode: state.process_inode,
        unit_snapshot,
        prior_native_state: state.state,
        prior_last_exit_code: state.last_exit_code,
        prior_restart: state.restart,
        prior_triggers: state.triggers,
        forward_object_recovery: None,
        rollback_object_recovery: None,
        status: if pending { "pending" } else { "stopped" }.to_string(),
        restored_pid: None,
        restored_started_at: None,
        restored_loaded_environment: BTreeMap::new(),
        restored_executable: None,
        restored_sha256: None,
        restored_device: None,
        restored_inode: None,
        restored_route: None,
    }))
}
