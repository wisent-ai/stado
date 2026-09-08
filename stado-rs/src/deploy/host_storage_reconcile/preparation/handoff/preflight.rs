use super::*;

fn physical_file_identity<'a>(
    preflight: &'a Value,
    inventory: &str,
    path: &str,
) -> Option<&'a Value> {
    preflight
        .get(inventory)?
        .get("files")?
        .as_array()?
        .iter()
        .find(|item| item.get("path").and_then(Value::as_str) == Some(path))
        .and_then(|item| item.get("body"))
}

pub(in crate::deploy::host_storage_reconcile) fn capture_storage_roots(
    transaction: &str,
    runtime: Value,
    writer: &WriterFence,
    staged: &crate::deploy::host_release::StagedRelease,
) -> Result<StorageRoots, DeployError> {
    let directory = transaction_directory(transaction)?;
    let home = directory.ancestors().nth(3).ok_or_else(|| {
        DeployError("transaction directory has no Stado data directory".to_string())
    })?;
    let primary = home.join("local-storage").to_string_lossy().into_owned();
    let backup = home.join("local-backup").to_string_lossy().into_owned();
    let storage = runtime.get("storage").ok_or_else(|| {
        DeployError("object API state omitted its constructed storage handle".to_string())
    })?;
    let pid = storage.get("pid").and_then(Value::as_u64);
    if pid != writer.prior_pid.as_deref().and_then(|pid| pid.parse().ok())
        || writer.prior_sha256.as_deref() != Some(staged.staged_sha256.as_str())
    {
        return Err(DeployError(format!(
            "object API identity differs from the captured process or staged declared runtime: \
             API PID {pid:?}, captured PID {:?}, mapped SHA-256 {:?}, staged SHA-256 {}",
            writer.prior_pid, writer.prior_sha256, staged.staged_sha256,
        )));
    }
    if storage.get("backend").and_then(Value::as_str) != Some("local")
        || storage
            .pointer("/write_fence/protocol")
            .and_then(Value::as_str)
            != Some(crate::queue::LocalBackend::WRITE_FENCE_PROTOCOL)
    {
        return Err(DeployError(
            "object API does not report the local storage write-fence protocol; \
             the declared release must converge before a storage handoff"
                .to_string(),
        ));
    }
    let prior_primary = storage
        .get("local_path")
        .and_then(Value::as_str)
        .filter(|path| *path == primary || *path == backup)
        .ok_or_else(|| {
            DeployError(format!(
                "object API constructed root {:?} is outside fixed roots {primary:?} and {backup:?}",
                storage.get("local_path")
            ))
        })?
        .to_string();
    let prior_backup = match storage.get("backup").filter(|value| !value.is_null()) {
        None => None,
        Some(mirror) => {
            let path = mirror
                .get("local_path")
                .and_then(Value::as_str)
                .filter(|path| *path == primary || *path == backup);
            if mirror.get("backend").and_then(Value::as_str) != Some("local")
                || path.is_none()
                || path == Some(prior_primary.as_str())
            {
                return Err(DeployError(format!(
                    "object API constructed mirror is outside the distinct fixed A/B roots: {mirror}"
                )));
            }
            path.map(str::to_string)
        }
    };
    Ok(StorageRoots {
        primary,
        backup,
        prior_primary,
        prior_backup,
        runtime,
    })
}

pub(in crate::deploy::host_storage_reconcile) async fn capture_fenced_preflight(
    target: &crate::targets::ComputeTarget,
    transaction: &str,
    fence: &mut LifecycleFence,
    runner: &Runner,
) -> Result<(), DeployError> {
    if fence.preflight_evidence.is_some() {
        return Ok(());
    }
    let mut preflight = remote_phase(target, transaction, PREFLIGHT, runner).await?;
    let writer = fence
        .writers
        .iter()
        .find(|writer| writer.role == "object-api")
        .ok_or_else(|| DeployError("fence omitted its object API".to_string()))?;
    let roots = fence.roots.as_ref().unwrap();
    let prior_root = if roots.prior_primary == roots.primary {
        "A"
    } else {
        "B"
    };
    let conflict_winner = if prior_root == "A" {
        "primary"
    } else {
        "backup"
    };
    let correlation = correlate_served_store(
        target,
        writer
            .listener_port
            .ok_or_else(|| DeployError("object API port is absent".to_string()))?,
        &preflight,
        false,
        conflict_winner,
        runner,
    )
    .await?;
    let authority = correlation
        .get("object_authority")
        .and_then(Value::as_str)
        .ok_or_else(|| DeployError("fenced API proof omitted its authority".to_string()))?;
    if !matches!(authority, "identical") && authority != prior_root {
        return Err(DeployError(
            "fenced API bytes disagree with its constructed storage root".to_string(),
        ));
    }
    let inventory = if prior_root == "A" {
        "primary_physical"
    } else {
        "backup_physical"
    };
    let configuration = json!({
        "object_api": {
            "runtime": roots.runtime,
            "observed_loaded_environment": writer.prior_loaded_environment,
            "unit_declaration": writer.unit_declared_environment,
            "registry_declaration": writer.registry_declared_environment,
        },
        "dashboard_registry_store": {
            "backend": "local", "namespace": Value::Null, "key": "registry.json",
            "physical_root": prior_root,
            "identity": physical_file_identity(&preflight, inventory, "registry.json"),
        },
    });
    let report = preflight
        .as_object_mut()
        .ok_or_else(|| DeployError("fenced preflight report is not an object".to_string()))?;
    report.insert("served_store".to_string(), correlation);
    report.insert("effective_configuration".to_string(), configuration);
    fence.preflight_evidence = Some(write_json_evidence(
        transaction,
        PREFLIGHT_EVIDENCE_FILE,
        &preflight,
        "fenced preflight evidence",
        true,
    )?);
    write_fence(target, transaction, fence, runner).await
}
