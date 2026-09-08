use super::*;

pub(super) async fn read_operation_owner(
    target: &crate::targets::ComputeTarget,
    transaction: &str,
    runner: &Runner,
) -> Result<Option<Value>, DeployError> {
    let output =
        host_channel::run_script(target, &bind_remote_script(READ_OWNER, transaction), runner)
            .await?;
    if !output.ok() {
        return Err(DeployError(remote_failure_detail(
            &output,
            "operation owner could not be read",
        )));
    }
    for line in output.stdout.lines() {
        if let Some(message) = line.strip_prefix("STADO_STORAGE_RECONCILE_ERROR\t") {
            return Err(DeployError(message.to_string()));
        }
        let Some(encoded) = line.strip_prefix("STADO_RECONCILE_OWNER\t") else {
            continue;
        };
        if encoded == "absent" {
            return Ok(None);
        }
        let mut owner: Value = serde_json::from_str(encoded)
            .map_err(|error| DeployError(format!("operation owner is invalid: {error}")))?;
        let label = owner
            .pointer("/native_manager/service")
            .and_then(Value::as_str)
            .ok_or_else(|| DeployError("operation owner omitted its native service".to_string()))?;
        let scope = match owner
            .pointer("/native_manager/domain")
            .and_then(Value::as_str)
        {
            Some("system") => service::BootoutScope::System,
            Some(domain) if domain.starts_with("gui/") || domain.starts_with("user/") => {
                service::BootoutScope::User
            }
            _ => service::BootoutScope::Any,
        };
        let observed =
            crate::deploy::service_label_print::print_label(target, label, scope, runner).await;
        let recorded_status = owner.get("status").cloned().unwrap_or(Value::Null);
        let executing = recorded_status.as_str() == Some("executing");
        let (observation, effective_status) = match observed {
            Ok(state) => {
                let owner_running = state
                    .pid
                    .as_deref()
                    .and_then(|pid| pid.parse::<u64>().ok())
                    .is_some_and(|pid| owner.get("pid").and_then(Value::as_u64) == Some(pid));
                let effective_status = if !executing {
                    recorded_status.clone()
                } else if state.unsupported.is_some() {
                    json!("unobserved")
                } else if owner_running {
                    recorded_status.clone()
                } else {
                    json!("interrupted")
                };
                (
                    json!({
                        "observed_at": Utc::now().to_rfc3339(),
                        "loaded": state.loaded(),
                        "domain": state.domain,
                        "pid": state.pid,
                        "state": state.state,
                        "last_exit_code": state.last_exit_code,
                        "unsupported": state.unsupported,
                    }),
                    effective_status,
                )
            }
            Err(error) => (
                json!({
                    "observed_at": Utc::now().to_rfc3339(),
                    "error": error.to_string(),
                }),
                if executing {
                    json!("unobserved")
                } else {
                    recorded_status.clone()
                },
            ),
        };
        let fields = owner
            .as_object_mut()
            .ok_or_else(|| DeployError("operation owner is not an object".to_string()))?;
        fields.insert("recorded_status".to_string(), recorded_status);
        fields.insert("status".to_string(), effective_status);
        fields.insert("native_manager_observation".to_string(), observation);
        return Ok(Some(owner));
    }
    Err(DeployError(
        "operation owner reader returned no marker".to_string(),
    ))
}

pub(super) fn read_captured_resident_target(
    target_name: &str,
    transaction: &str,
) -> Result<Option<crate::targets::ComputeTarget>, DeployError> {
    let directory = transaction_directory(transaction)?;
    for (name, schema) in [
        ("operation-owner.json", "stado.storage-root-owner.v1"),
        ("launch-intent.json", "stado.storage-root-launch.v1"),
    ] {
        let path = directory.join(name);
        let metadata = match std::fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => {
                return Err(DeployError(format!(
                    "cannot inspect captured resident target {}: {error}",
                    path.display()
                )));
            }
        };
        if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
            return Err(DeployError(format!(
                "captured resident target receipt is not a regular file: {}",
                path.display()
            )));
        }
        let receipt: Value = serde_json::from_slice(&std::fs::read(&path).map_err(|error| {
            DeployError(format!(
                "cannot read captured resident target {}: {error}",
                path.display()
            ))
        })?)
        .map_err(|error| {
            DeployError(format!(
                "captured resident target receipt {} is invalid: {error}",
                path.display()
            ))
        })?;
        if receipt.get("schema").and_then(Value::as_str) != Some(schema)
            || receipt.get("transaction").and_then(Value::as_str) != Some(transaction)
            || receipt.get("target").and_then(Value::as_str) != Some(target_name)
        {
            return Err(DeployError(format!(
                "captured resident target receipt {} has the wrong identity",
                path.display()
            )));
        }
        let target: crate::targets::ComputeTarget =
            serde_json::from_value(receipt.get("target_config").cloned().ok_or_else(|| {
                DeployError(format!(
                    "captured resident target receipt {} omitted target_config",
                    path.display()
                ))
            })?)
            .map_err(|error| {
                DeployError(format!("captured resident target is invalid: {error}"))
            })?;
        if target.name != target_name || !host_channel::target_is_this_host(&target) {
            return Err(DeployError(
                "captured resident target does not identify this host".to_string(),
            ));
        }
        return Ok(Some(target));
    }
    Ok(None)
}
