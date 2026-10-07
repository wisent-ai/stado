use super::*;

pub(in crate::deploy::host_storage_reconcile) async fn prove_listener_closed(
    target: &crate::targets::ComputeTarget,
    port: u16,
    runner: &Runner,
) -> Result<(), DeployError> {
    let script = host_step_script(&["listener-closed", "--port", &port.to_string()])?;
    let output = host_channel::run_script(target, &script, runner).await?;
    let marker = format!("STADO_LISTENER_CLOSED\t{port}");
    if !output.ok() || !output.stdout.lines().any(|line| line == marker) {
        return Err(DeployError(format!(
            "object API listener on {}:{port} did not close: {}",
            target.name,
            remote_failure_detail(&output, "remote command failed")
        )));
    }
    Ok(())
}

pub(in crate::deploy::host_storage_reconcile) async fn snapshot_unit_file(
    target: &crate::targets::ComputeTarget,
    path: &str,
    runner: &Runner,
) -> Result<Option<FileSnapshot>, DeployError> {
    let script = host_step_script(&["unit-snapshot", "--path", path])?;
    let output = host_channel::run_script(target, &script, runner).await?;
    if !output.ok() {
        return Err(DeployError::unreachable(format!(
            "unit snapshot failed for {path} on {}: {}",
            target.name,
            remote_failure_detail(&output, "remote command failed")
        )));
    }
    let value = output
        .stdout
        .lines()
        .find_map(|line| line.strip_prefix("STADO_UNIT_SNAPSHOT\t"))
        .ok_or_else(|| DeployError::unreachable("unit snapshot returned no marker".to_string()))?;
    if value == "absent" {
        return Ok(None);
    }
    serde_json::from_str(value)
        .map(Some)
        .map_err(|error| DeployError::unreachable(format!("unit snapshot is invalid: {error}")))
}
pub(in crate::deploy::host_storage_reconcile) fn unit_declared_environment(
    candidate: &ServiceCandidate,
    snapshot: Option<&FileSnapshot>,
) -> Result<BTreeMap<String, String>, DeployError> {
    let Some(snapshot) = snapshot else {
        return Ok(BTreeMap::new());
    };
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(&snapshot.body_base64)
        .map_err(|error| {
            DeployError::unreachable(format!("unit snapshot base64 is invalid: {error}"))
        })?;
    let content = String::from_utf8(bytes)
        .map_err(|error| DeployError::unreachable(format!("unit snapshot is not UTF-8: {error}")))?;
    let kind = if candidate.declared.path.ends_with(".service") {
        service::KIND_SYSTEMD
    } else {
        service::KIND_LAUNCHD
    };
    let unit = service::UnitFile {
        host: candidate.target.name.clone(),
        unit: candidate.declared.unit_id().to_string(),
        path: candidate.declared.path.clone(),
        kind,
        content,
    };
    let parsed = service::unit_environment(&unit)?;
    Ok(parsed.env.into_iter().collect())
}
