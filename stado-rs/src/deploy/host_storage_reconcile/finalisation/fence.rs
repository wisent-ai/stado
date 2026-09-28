use super::*;

/// One phase of the host program, run by the transaction's own tool (the
/// binary this operator staged, so both sides are one revision). A host
/// that never launched the transaction has no tool; its reads go to the
/// installed Stado, which answers them from the same program.
pub(in crate::deploy::host_storage_reconcile) fn bind_remote_script(
    phase: &str,
    transaction: &str,
) -> String {
    let owner_token = RESIDENT_OWNER_TOKEN.get().map(String::as_str).unwrap_or("");
    let lock_fd = RESIDENT_LOCK_FD.get().copied().unwrap_or(-1).to_string();
    format!(
        r#"set -u
tool="$HOME/.stado/recovery/storage-root-reconcile/"{transaction_path}/transaction-tool
[ -x "$tool" ] || tool="$HOME/.stado/bin/stado"
STADO_RECONCILE_OWNER_TOKEN={owner_token} STADO_RECONCILE_LOCK_FD={lock_fd} \
  "$tool" host storage-root-reconcile-local --phase {phase} --transaction {transaction} 2>&1
"#,
        transaction_path = shlex_quote(transaction),
        owner_token = shlex_quote(owner_token),
        lock_fd = shlex_quote(&lock_fd),
        phase = shlex_quote(phase),
        transaction = shlex_quote(transaction),
    )
}

/// One host step (`stado host storage-root-reconcile-host ARGUMENTS`), run
/// by the resident worker's own binary: the worker is the transaction tool
/// on this host, so the step is the same revision as the transaction.
pub(in crate::deploy::host_storage_reconcile) fn host_step_script(
    arguments: &[&str],
) -> Result<String, DeployError> {
    let tool = std::env::current_exe()
        .map_err(|error| DeployError(format!("cannot locate the transaction tool: {error}")))?;
    let mut script = format!(
        "{} host storage-root-reconcile-host",
        shlex_quote(&tool.to_string_lossy())
    );
    for argument in arguments {
        script.push(' ');
        script.push_str(&shlex_quote(argument));
    }
    Ok(script)
}

pub(in crate::deploy::host_storage_reconcile) fn remote_failure_detail(
    output: &crate::deploy::CommandOutput,
    fallback: &str,
) -> String {
    let stdout = output.stdout.trim();
    let stderr = output.stderr.trim();
    match (stdout.is_empty(), stderr.is_empty()) {
        (true, true) => fallback.to_string(),
        (false, true) => stdout.to_string(),
        (true, false) => stderr.to_string(),
        (false, false) => format!("{stdout}\n{stderr}"),
    }
}

pub(in crate::deploy::host_storage_reconcile) fn parse_remote_payload(
    output: &crate::deploy::CommandOutput,
) -> Result<Value, DeployError> {
    let mut payload = None;
    for line in output.stdout.lines() {
        if let Some(message) = line.strip_prefix("STADO_STORAGE_RECONCILE_ERROR\t") {
            return Err(DeployError(message.to_string()));
        }
        if let Some(encoded) = line.strip_prefix("STADO_STORAGE_RECONCILE\t") {
            payload = serde_json::from_str(encoded).ok();
        }
    }
    if !output.ok() {
        return Err(DeployError(remote_failure_detail(
            output,
            "storage reconciliation host program failed",
        )));
    }
    payload.ok_or_else(|| DeployError("storage reconciliation returned no payload".to_string()))
}

pub(in crate::deploy::host_storage_reconcile) async fn read_fence(
    target: &crate::targets::ComputeTarget,
    transaction: &str,
    runner: &Runner,
) -> Result<Option<LifecycleFence>, DeployError> {
    let output = host_channel::run_script_with_timeout(
        target,
        &bind_remote_script(READ_FENCE, transaction),
        TIMEOUT,
        runner,
    )
    .await?;
    let value = parse_remote_payload(&output)?;
    if value.get("status").and_then(Value::as_str) == Some("absent") {
        return Ok(None);
    }
    serde_json::from_value(value)
        .map(Some)
        .map_err(|error| DeployError(format!("invalid durable lifecycle fence: {error}")))
}

pub(in crate::deploy::host_storage_reconcile) async fn write_fence(
    target: &crate::targets::ComputeTarget,
    transaction: &str,
    fence: &LifecycleFence,
    _runner: &Runner,
) -> Result<(), DeployError> {
    if !host_channel::target_is_this_host(target) {
        return Err(DeployError(
            "lifecycle fence can only be written by the resident target worker".to_string(),
        ));
    }
    if fence.schema != FENCE_SCHEMA || fence.transaction != transaction {
        return Err(DeployError(
            "lifecycle fence belongs to another transaction".to_string(),
        ));
    }
    verify_resident_lock(transaction)?;
    atomic_json_file(
        &transaction_directory(transaction)?.join("lifecycle-fence.json"),
        fence,
        "lifecycle fence",
    )
}
pub(in crate::deploy::host_storage_reconcile) async fn refresh_resident_owner(
    target: &crate::targets::ComputeTarget,
    transaction: &str,
    fence: &mut LifecycleFence,
    runner: &Runner,
) -> Result<(), DeployError> {
    let current = resident_owner_retention(transaction)?;
    if fence.resident_owner != current {
        fence.resident_owner = current;
        write_fence(target, transaction, fence, runner).await?;
    }
    Ok(())
}

pub(in crate::deploy::host_storage_reconcile) fn validate_transaction(
    transaction: &str,
) -> Result<(), DeployError> {
    if transaction.is_empty()
        || transaction.len() > 96
        || !transaction
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
    {
        return Err(DeployError(
            "transaction must contain 1-96 ASCII letters, digits, or '-'".to_string(),
        ));
    }
    Ok(())
}

pub(in crate::deploy::host_storage_reconcile) async fn remote_phase(
    target: &crate::targets::ComputeTarget,
    transaction: &str,
    phase: &str,
    runner: &Runner,
) -> Result<Value, DeployError> {
    let output = host_channel::run_script_with_timeout(
        target,
        &bind_remote_script(phase, transaction),
        TIMEOUT,
        runner,
    )
    .await?;
    parse_remote_payload(&output)
}
