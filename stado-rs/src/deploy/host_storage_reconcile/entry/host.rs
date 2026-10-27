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
        if !status.is_object() {
            return Err(DeployError(format!(
                "{target_name}: the storage-root status report is not an object: {status}"
            ))
            .stating(crate::primitives::failure::FailureCode::InfraDown));
        }
        // Serde writes a transaction nobody owns as `null`.
        status["operation_owner"] = serde_json::json!(owner);
        return Ok(status);
    }
    if !matches!(phase, RUN | RESUME | ROLLBACK | FINALIZE) {
        return Err(DeployError(format!(
            "action must be {RUN}, {RESUME}, {STATUS}, {ROLLBACK}, or {FINALIZE}"
        ))
        .stating(crate::primitives::failure::FailureCode::Refused));
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
        )
        .stating(crate::primitives::failure::FailureCode::Refused));
    }
    let executable = std::env::current_exe().map_err(DeployError::io(
        "cannot locate transaction tool".to_string(),
    ))?;
    let tool_bytes = std::fs::read(&executable)
        .map_err(DeployError::io("cannot read transaction tool".to_string()))?;
    let tool_sha256 = hex::encode(Sha256::digest(&tool_bytes));
    let work = format!("$HOME/.stado/recovery/storage-root-reconcile/{transaction}");
    let staged_tool = format!("{work}/transaction-tool.{tool_sha256}");
    let canonical_tool = format!("{work}/transaction-tool");
    let staged = service::sync_service_file(
        &target,
        &staged_tool,
        &tool_bytes,
        crate::primitives::file_mode::owner_program(),
        runner,
    )
    .await?;
    if !staged.succeeded("file_synced") {
        return Err(DeployError::unreachable(format!(
            "transaction tool staging failed: {}",
            staged.failure()
        )));
    }
    // A transaction without a runner gate sends the worker an empty gate
    // argument, which the worker reads as no gate.
    let runner_gate = match runner_gate {
        Some(gate) => {
            let bytes = serde_json::to_vec(&gate)
                .map_err(|error| DeployError(format!("cannot encode runner gate: {error}")))?;
            base64::engine::general_purpose::STANDARD.encode(bytes)
        }
        None => String::new(),
    };
    let target_config = base64::engine::general_purpose::STANDARD.encode(
        serde_json::to_vec(&target)
            .map_err(|error| DeployError(format!("cannot encode resident target: {error}")))?,
    );
    let arguments = vec![
        "host".to_string(),
        "local".to_string(),
        "reconcile".to_string(),
        "worker".to_string(),
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
    if !launched.succeeded() {
        return Err(DeployError::unreachable(host_channel::last_error_line(
            &launched,
            "resident reconciliation worker did not launch",
        )));
    }
    // The worker's owner line is its acknowledgement; a line that is not JSON
    // is a damaged acknowledgement, said with its cause.
    let owner_line = launched
        .stdout
        .lines()
        .find_map(|line| line.strip_prefix("STADO_RECONCILE_OWNER\t"))
        .ok_or_else(|| {
            DeployError::unreachable("resident reconciliation worker reported no owner".to_string())
        })?;
    let owner: Value = serde_json::from_str(owner_line).map_err(|error| {
        DeployError::unreachable(format!(
            "resident reconciliation worker reported an owner that is not JSON: {error}"
        ))
    })?;
    let mut report = host_channel::base_report(&target);
    report.insert("transaction".to_string(), json!(transaction));
    report.insert("phase".to_string(), json!(phase));
    report.insert("status".to_string(), json!("accepted"));
    report.insert("operation_owner".to_string(), owner);
    Ok(Value::Object(report))
}
