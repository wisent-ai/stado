use super::*;
use crate::primitives::failure::FailureCode;

pub(super) async fn read_operation_owner(
    target: &crate::targets::ComputeTarget,
    transaction: &str,
    runner: &Runner,
) -> Result<Option<Value>, DeployError> {
    let output =
        host_channel::run_script(target, &bind_remote_script(READ_OWNER, transaction), runner)
            .await?;
    if !output.succeeded() {
        return Err(DeployError::unreachable(remote_failure_detail(
            &output,
            "operation owner could not be read",
        )));
    }
    for line in output.stdout.lines() {
        if let Some(message) = line.strip_prefix("STADO_STORAGE_RECONCILE_ERROR\t") {
            return Err(DeployError::unreachable(message.to_string()));
        }
        let Some(encoded) = line.strip_prefix("STADO_RECONCILE_OWNER\t") else {
            continue;
        };
        if encoded == "absent" {
            return Ok(None);
        }
        let mut owner: Value = serde_json::from_str(encoded).map_err(|error| {
            DeployError::unreachable(format!("operation owner is invalid: {error}"))
        })?;
        let label = owner
            .pointer("/native_manager/service")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                DeployError::unreachable("operation owner omitted its native service".to_string())
            })?;
        // The owner's manager is the host's one Stado unit, whose recorded
        // identity (finalisation/state/resident.rs) names its service, pids
        // and state but no launchd domain, so its label is printed in either
        // scope: system first, then the calling account (BootoutScope::Any).
        let scope = service::BootoutScope::Any;
        let observed =
            crate::deploy::service_label_print::print_label(target, label, scope, runner).await;
        // An owner without a recorded status reads as null, which no status
        // word equals; the index answers that for a missing member.
        let recorded_status = owner["status"].clone();
        let executing = recorded_status.as_str() == Some("executing");
        let (observation, effective_status) = match observed {
            Ok(state) => {
                // The owner is running only when the loaded unit names a pid
                // and it is the owner's recorded pid as a number prints; a
                // unit with no process (`-`) names no pid that could match.
                let recorded_pid = owner
                    .get("pid")
                    .and_then(Value::as_u64)
                    .map(|pid| pid.to_string());
                let owner_running = state.pid.is_some() && state.pid == recorded_pid;
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
        let fields = owner.as_object_mut().ok_or_else(|| {
            DeployError::unreachable("operation owner is not an object".to_string())
        })?;
        fields.insert("recorded_status".to_string(), recorded_status);
        fields.insert("status".to_string(), effective_status);
        fields.insert("native_manager_observation".to_string(), observation);
        return Ok(Some(owner));
    }
    Err(DeployError::unreachable(
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
                ))
                .stating(crate::cli::entry::error::io_failure_code(error.kind())));
            }
        };
        if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
            return Err(DeployError(format!(
                "captured resident target receipt is not a regular file: {}",
                path.display()
            ))
            .stating(FailureCode::InfraDown));
        }
        let receipt: Value = serde_json::from_slice(&std::fs::read(&path).map_err(|error| {
            DeployError(format!(
                "cannot read captured resident target {}: {error}",
                path.display()
            ))
            .stating(crate::cli::entry::error::io_failure_code(error.kind()))
        })?)
        .map_err(|error| {
            DeployError(format!(
                "captured resident target receipt {} is invalid: {error}",
                path.display()
            ))
            .stating(FailureCode::InfraDown)
        })?;
        if receipt.get("schema").and_then(Value::as_str) != Some(schema)
            || receipt.get("transaction").and_then(Value::as_str) != Some(transaction)
            || receipt.get("target").and_then(Value::as_str) != Some(target_name)
        {
            return Err(DeployError(format!(
                "captured resident target receipt {} has the wrong identity",
                path.display()
            ))
            .stating(FailureCode::InfraDown));
        }
        let target: crate::targets::ComputeTarget =
            serde_json::from_value(receipt.get("target_config").cloned().ok_or_else(|| {
                DeployError(format!(
                    "captured resident target receipt {} omitted target_config",
                    path.display()
                ))
                .stating(FailureCode::InfraDown)
            })?)
            .map_err(|error| {
                DeployError(format!("captured resident target is invalid: {error}"))
                    .stating(FailureCode::InfraDown)
            })?;
        if target.name != target_name || !host_channel::target_is_this_host(&target) {
            return Err(DeployError(
                "captured resident target does not identify this host".to_string(),
            )
            .stating(FailureCode::Refused));
        }
        return Ok(Some(target));
    }
    Ok(None)
}
