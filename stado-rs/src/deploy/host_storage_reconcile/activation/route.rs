use super::*;

#[allow(clippy::too_many_arguments)]
pub(super) async fn restored_object_route(
    storage_target: &crate::targets::ComputeTarget,
    fence: &LifecycleFence,
    index: usize,
    label: &str,
    state: &crate::deploy::service_label_print::LabelState,
    was_durably_restored: bool,
    rollback: bool,
    roots: &StorageRoots,
    preflight: Option<&Value>,
    conflict_winner: &str,
    runner: &Runner,
) -> Result<Option<Value>, DeployError> {
    let restored_route = if fence.writers[index].role == "object-api" && was_durably_restored {
        Some(fence.writers[index].restored_route.clone().ok_or_else(|| {
            DeployError("durable object API result omitted its route proof".to_string())
        })?)
    } else if fence.writers[index].role == "object-api" {
        let port = fence.writers[index].listener_port.ok_or_else(|| {
            DeployError("object API listener port is absent from its fence".to_string())
        })?;
        let runtime = observe_object_runtime(storage_target, port, runner).await?;
        let storage = runtime.get("storage").ok_or_else(|| {
            DeployError("restored object API omitted its constructed storage".to_string())
        })?;
        let (expected_root, expected_backup) = if rollback {
            (roots.prior_primary.as_str(), roots.prior_backup.as_deref())
        } else {
            (roots.primary.as_str(), Some(roots.backup.as_str()))
        };
        let mirror_matches = match expected_backup {
            Some(path) => {
                storage.pointer("/backup/backend").and_then(Value::as_str) == Some("local")
                    && storage
                        .pointer("/backup/local_path")
                        .and_then(Value::as_str)
                        == Some(path)
            }
            None => storage.get("backup").is_none_or(Value::is_null),
        };
        if storage.get("backend").and_then(Value::as_str) != Some("local")
            || storage.get("local_path").and_then(Value::as_str) != Some(expected_root)
            || storage.get("pid").and_then(Value::as_u64)
                != state.pid.as_deref().and_then(|pid| pid.parse().ok())
            || storage
                .pointer("/write_fence/protocol")
                .and_then(Value::as_str)
                != Some(crate::queue::LocalBackend::WRITE_FENCE_PROTOCOL)
            || !mirror_matches
        {
            return Err(DeployError(format!(
                "{label} constructed storage does not match its recorded recovery route: {storage}"
            )));
        }
        let mut correlation = if let Some(preflight) = preflight {
            correlate_served_store(
                storage_target,
                port,
                preflight,
                !rollback,
                conflict_winner,
                runner,
            )
            .await?
        } else {
            json!({
                "endpoint": format!("http://127.0.0.1:{port}"),
                "object_authority": if expected_root == roots.primary { "A" } else { "B" },
                "evidence": "constructed-runtime-without-data-mutation",
            })
        };
        correlation["runtime"] = runtime;
        let authority = correlation
            .get("object_authority")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let accepted = if rollback {
            matches!(authority, "identical")
                || authority
                    == if roots.prior_primary == roots.primary {
                        "A"
                    } else {
                        "B"
                    }
        } else {
            matches!(authority, "A" | "identical")
        };
        if !accepted {
            return Err(DeployError(format!(
                "{label} serves {authority:?} after {} recovery",
                if rollback {
                    "captured-prior"
                } else {
                    "forward A+B"
                }
            )));
        }
        let prepared_sha256 = if rollback {
            fence.writers[index].rollback_object_recovery.as_ref()
        } else {
            fence.writers[index].forward_object_recovery.as_ref()
        }
        .map(|script| script.sha256.clone());
        Some(json!({
            "configuration": {
                "prepared_script_sha256": prepared_sha256,
                "loaded_environment_observed": state
                    .loaded_environment
                    .contains_key("WC_STORAGE_BACKEND"),
                "observed_loaded_environment": state.loaded_environment.clone(),
                "unit_declared_environment":
                    fence.writers[index].unit_declared_environment.clone(),
                "registry_declared_environment":
                    fence.writers[index].registry_declared_environment.clone(),
            },
            "served_store": correlation,
        }))
    } else {
        None
    };
    Ok(restored_route)
}
