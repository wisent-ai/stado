use super::*;

pub(in crate::deploy::host_storage_reconcile) fn prepared_script(body: String) -> PreparedScript {
    PreparedScript {
        sha256: hex::encode(Sha256::digest(body.as_bytes())),
        body,
    }
}
pub(in crate::deploy::host_storage_reconcile) async fn correlate_served_store(
    target: &crate::targets::ComputeTarget,
    port: u16,
    preflight: &Value,
    primary_after_commit: bool,
    conflict_winner: &str,
    runner: &Runner,
) -> Result<Value, DeployError> {
    if !matches!(conflict_winner, "primary" | "backup") {
        return Err(DeployError(
            "served-store correlation conflict winner is invalid".to_string(),
        ));
    }
    let payload = serde_json::to_vec(&json!({
        "primary": preflight.get("primary_qualified"),
        "backup": preflight.get("backup_qualified"),
        "primary_physical": preflight.get("primary_physical"),
        "backup_physical": preflight.get("backup_physical"),
        "primary_after_commit": primary_after_commit,
        "conflict_winner": conflict_winner,
    }))
    .map_err(|error| DeployError(format!("cannot encode served-store inventory: {error}")))?;
    let encoded = base64::engine::general_purpose::STANDARD.encode(payload);
    let command = host_step_script(&["served-store", "--port", &port.to_string()])?;
    let script = format!("printf '%s' {} | {command}", shlex_quote(&encoded));
    let output = host_channel::run_script(target, &script, runner).await?;
    if !output.ok() {
        return Err(DeployError(format!(
            "object API physical-store correlation failed on {}:{port}: {}",
            target.name,
            remote_failure_detail(&output, "remote command failed")
        )));
    }
    output
        .stdout
        .lines()
        .find_map(|line| line.strip_prefix("STADO_SERVED_STORE\t"))
        .ok_or_else(|| DeployError("object API correlation returned no evidence".to_string()))
        .and_then(|body| {
            serde_json::from_str(body)
                .map_err(|error| DeployError(format!("object API correlation is invalid: {error}")))
        })
}
pub(in crate::deploy::host_storage_reconcile) async fn observe_object_runtime(
    target: &crate::targets::ComputeTarget,
    port: u16,
    runner: &Runner,
) -> Result<Value, DeployError> {
    let script = host_step_script(&["object-runtime", "--port", &port.to_string()])?;
    let output = host_channel::run_script(target, &script, runner).await?;
    parse_remote_payload(&output)
}
