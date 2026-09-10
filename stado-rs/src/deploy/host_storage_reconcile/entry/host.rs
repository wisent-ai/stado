use super::*;

pub async fn reconcile_host(
    target_name: &str,
    transaction: &str,
    phase: &str,
    runner: &Runner,
) -> Result<Value, DeployError> {
    validate_transaction(transaction)?;
    let target = if matches!(phase, STATUS | RESUME | ROLLBACK | FINALIZE) {
        match read_captured_resident_target(target_name, transaction)? {
            Some(target) => target,
            None => host_channel::canonical_target(target_name).await?,
        }
    } else {
        host_channel::canonical_target(target_name).await?
    };
    if phase == STATUS {
        let mut status = reconcile_host_inner(&target, transaction, STATUS, runner).await?;
        let owner = read_operation_owner(&target, transaction, runner).await?;
        status
            .as_object_mut()
            .expect("storage-root status report is an object")
            .insert("operation_owner".to_string(), owner.unwrap_or(Value::Null));
        return Ok(status);
    }
    if !matches!(phase, RUN | RESUME | ROLLBACK | FINALIZE) {
        return Err(DeployError(format!(
            "action must be {RUN}, {RESUME}, {STATUS}, {ROLLBACK}, or {FINALIZE}"
        )));
    }
    let runner_gate = if matches!(phase, RUN | RESUME) {
        repository_runner_gate().await?
    } else {
        None
    };
    if runner_gate.as_ref().is_some_and(|gate| {
        gate.get("source_sha").and_then(Value::as_str)
            != Some(crate::binary::build_identity::SOURCE_REVISION)
    }) {
        return Err(DeployError(
            "current GitHub job source differs from the transaction tool source".to_string(),
        ));
    }
    let executable = std::env::current_exe()
        .map_err(|error| DeployError(format!("cannot locate transaction tool: {error}")))?;
    let tool_bytes = std::fs::read(&executable)
        .map_err(|error| DeployError(format!("cannot read transaction tool: {error}")))?;
    let tool_sha256 = hex::encode(Sha256::digest(&tool_bytes));
    let work = format!("$HOME/.stado/recovery/storage-root-reconcile/{transaction}");
    let staged_tool = format!("{work}/transaction-tool.{tool_sha256}");
    let canonical_tool = format!("{work}/transaction-tool");
    let staged =
        service::sync_service_file(&target, &staged_tool, &tool_bytes, 0o700, runner).await?;
    if !staged.succeeded("file_synced") {
        return Err(DeployError(format!(
            "transaction tool staging failed: {}",
            staged.failure()
        )));
    }
    let runner_gate = runner_gate
        .map(|gate| serde_json::to_vec(&gate))
        .transpose()
        .map_err(|error| DeployError(format!("cannot encode runner gate: {error}")))?
        .map(|bytes| base64::engine::general_purpose::STANDARD.encode(bytes))
        .unwrap_or_default();
    let target_config = base64::engine::general_purpose::STANDARD.encode(
        serde_json::to_vec(&target)
            .map_err(|error| DeployError(format!("cannot encode resident target: {error}")))?,
    );
    let arguments = vec![
        "host".to_string(),
        "storage-root-reconcile-worker".to_string(),
        target_name.to_string(),
        "--target-config".to_string(),
        target_config,
        "--transaction".to_string(),
        transaction.to_string(),
        "--phase".to_string(),
        phase.to_string(),
        "--source-revision".to_string(),
        crate::binary::build_identity::SOURCE_REVISION.to_string(),
        "--tool-sha256".to_string(),
        tool_sha256.clone(),
        "--runner-gate".to_string(),
        runner_gate,
    ];
    let launched = host_channel::run_script(
        &target,
        &launch_worker_script(
            transaction,
            &staged_tool,
            &canonical_tool,
            &tool_sha256,
            &arguments,
        )?,
        runner,
    )
    .await?;
    if !launched.ok() {
        return Err(DeployError(host_channel::last_error_line(
            &launched,
            "resident reconciliation worker did not launch",
        )));
    }
    let owner = launched
        .stdout
        .lines()
        .find_map(|line| {
            line.strip_prefix("STADO_RECONCILE_OWNER\t")
                .and_then(|value| serde_json::from_str::<Value>(value).ok())
        })
        .ok_or_else(|| {
            DeployError("resident reconciliation worker reported no owner".to_string())
        })?;
    let mut report = host_channel::base_report(&target);
    report.insert("transaction".to_string(), json!(transaction));
    report.insert("phase".to_string(), json!(phase));
    report.insert("status".to_string(), json!("accepted"));
    report.insert("operation_owner".to_string(), owner);
    Ok(Value::Object(report))
}
