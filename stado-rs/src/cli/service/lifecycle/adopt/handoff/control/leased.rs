//! `service handoff-release-control`: the half that runs under the unit's
//! mutation lease, from the release-state read to the sole registry CAS.

use super::*;

pub(super) async fn handoff_under_lease(context: HandoffContext<'_>) -> Result<(), CmdError> {
    let HandoffContext {
        mut document,
        expected_generation,
        target,
        service_name,
        host,
        product,
        json_output,
        profile_name,
        target_policy,
        desired,
        desired_artifact,
        receipt_path,
        prior_receipt,
        legacy_label,
        legacy_plist,
        legacy,
    } = context;
    let runner = production_runner();
    let state_path = crate::release_agent::host_state_path(&target_policy.state_dir, product);
    let state_text = host_channel::remote_read_file(&target, &state_path, &runner)
        .await
        .map_err(click)?
        .ok_or_else(|| {
            CmdError::click(format!("{host}: release state is absent at {state_path}"))
        })?;
    let state = crate::release_agent::parse_state_document(
        state_text.as_bytes(),
        product,
        host,
        &state_path,
    )
    .map_err(CmdError::click)?;
    let active = state.active.as_ref().ok_or_else(|| {
        CmdError::click(format!(
            "{host}: release-control has no active {product:?} process"
        ))
    })?;
    if state.phase != crate::release_agent::RolloutPhase::Committed
        || state.candidate.is_some()
        || state.proxy_pid.is_none()
        || state.rollout_generation != desired.rollout_generation
        || active.version != desired.version
        || active.artifact_sha256 != desired_artifact.artifact_sha256
        || active.manifest_sha256 != desired_artifact.manifest_sha256
    {
        return Err(CmdError::click(format!(
            "{host}: release-control {product:?} is not the settled desired release"
        )));
    }

    let installed_stado = format!("{}/.stado/bin/stado", target_policy.home);
    let active_binary = host_channel::run_program(
        &target,
        &[
            &installed_stado,
            "release",
            "active-binary",
            product,
            "--target",
            host,
            "--json",
        ],
        &runner,
    )
    .await
    .map_err(click)?;
    if !active_binary.ok() {
        return Err(CmdError::click(format!(
            "{host}: installed Stado rejected active release binary: {}",
            host_channel::last_error_line(&active_binary, "active-binary failed")
        )));
    }
    let active_binary: Value =
        serde_json::from_str(active_binary.stdout.trim()).map_err(|error| {
            CmdError::click(format!(
                "{host}: active-binary returned invalid JSON: {error}"
            ))
        })?;
    if active_binary["state"] != "active"
        || active_binary["product"] != product
        || active_binary["target"] != host
        || active_binary["version"] != desired.version
        || active_binary["artifact_sha256"] != desired_artifact.artifact_sha256
        || active_binary["manifest_sha256"] != desired_artifact.manifest_sha256
    {
        return Err(CmdError::click(format!(
            "{host}: active-binary identity does not match the desired release"
        )));
    }

    let serving = target_policy
        .blue_green_serving()
        .map_err(CmdError::click)?;
    let readiness_url = format!("http://{}{}", serving.stable_bind, serving.readiness_path);
    let readiness = host_channel::run_program(
        &target,
        &[
            "/usr/bin/curl",
            "--silent",
            "--show-error",
            "--fail",
            "--max-time",
            "3",
            "--output",
            "/dev/null",
            &readiness_url,
        ],
        &runner,
    )
    .await
    .map_err(click)?;
    if !readiness.ok() {
        return Err(CmdError::click(format!(
            "{host}: release-control readiness failed at {readiness_url}: {}",
            host_channel::last_error_line(&readiness, "readiness request failed")
        )));
    }
    let label = service_label_print::print_label(
        &target,
        legacy_label,
        service::BootoutScope::System,
        &runner,
    )
    .await
    .map_err(click)?;
    if label.loaded() {
        return Err(CmdError::click(format!(
            "{host}: legacy launchd label {legacy_label:?} is still loaded"
        )));
    }
    require_no_executable_caller(&target, &legacy.program, &runner).await?;
    let observed_plist_identity = remote_file_identity(&target, legacy_plist, &runner).await?;
    let observed_binary_identity = remote_file_identity(&target, &legacy.program, &runner).await?;
    let (plist_identity, binary_identity) = if let Some(receipt) = prior_receipt.as_ref() {
        let stored_plist = &receipt["retirement"]["plist_receipt"];
        let stored_binary = &receipt["retirement"]["binary_receipt"];
        if !same_remote_file_identity(stored_plist, &observed_plist_identity)
            || !same_remote_file_identity(stored_binary, &observed_binary_identity)
        {
            return Err(CmdError::click(format!(
                "handoff receipt {} no longer matches the exact legacy files",
                receipt_path.display()
            )));
        }
        (stored_plist.clone(), stored_binary.clone())
    } else {
        (observed_plist_identity, observed_binary_identity)
    };

    service::remove_service(&mut document, host, legacy_label).map_err(click)?;
    externalize_release_controlled_profile(&mut document, profile_name, service_name, product)?;
    remove_release_legacy_identity(&mut document, product, host)?;
    crate::service_resolution::advance_generation(&mut document).map_err(CmdError::click)?;
    for obsolete in [legacy_label, legacy_plist, legacy.program.as_str()] {
        if obsolete.is_empty() || document_contains_string(&document, obsolete) {
            return Err(CmdError::click(format!(
                "registry handoff left obsolete executable identity {obsolete:?} reachable"
            )));
        }
    }
    crate::targets::validate_registry(&document)
        .map_err(|error| CmdError::click(error.to_string()))?;

    let mut report = if let Some(receipt) = prior_receipt.as_ref() {
        let mut receipt = receipt.clone();
        if receipt["status"] == "registry_committed" {
            let recovery = json!({
                "recorded_generation": receipt["generation"],
                "recorded_expected_generation": receipt["expected_generation"],
                "observed_registry_generation": expected_generation,
                "revalidated_at": now(),
            });
            if receipt["registry_recovery_history"].is_null() {
                receipt["registry_recovery_history"] = json!([]);
            }
            receipt["registry_recovery_history"]
                .as_array_mut()
                .ok_or_else(|| {
                    CmdError::click(format!(
                        "handoff receipt {} has invalid registry recovery history",
                        receipt_path.display()
                    ))
                })?
                .push(recovery);
        }
        receipt["expected_generation"] = json!(expected_generation);
        receipt["generation"] = Value::Null;
        receipt["status"] = json!("prepared");
        receipt["reconciler_fence"] = json!({"status": "not_captured"});
        receipt
    } else {
        json!({
            "schema": "stado.service-release-control-handoff.v1",
            "status": "prepared",
            "service": service_name,
            "host": host,
            "profile": profile_name,
            "controller": "release-control",
            "product": product,
            "expected_generation": expected_generation,
            "generation": Value::Null,
            "receipt_path": receipt_path,
            "intended_post_handoff": {
                "controller": "release-control",
                "product": product,
                "legacy_registry_identities_removed": true,
            },
            "release": {
                "version": active.version,
                "rollout_generation": state.rollout_generation,
                "artifact_sha256": active.artifact_sha256,
                "manifest_sha256": active.manifest_sha256,
                "proxy_pid": state.proxy_pid,
                "active_binary": active_binary["path"],
                "readiness_url": readiness_url,
            },
            "reconciler_fence": {
                "status": "not_captured",
            },
            "legacy": {
                "label": legacy_label,
                "loaded": false,
                "registry_referrers": [],
            },
            "retirement": {
                "status": "pending",
                "order": ["plist", "binary"],
                "plist_receipt": plist_identity,
                "binary_receipt": binary_identity,
            },
        })
    };
    persist_handoff_receipt(&receipt_path, &report, prior_receipt.is_some())?;
    let generation = registry::push_document_if(&document, &expected_generation).await?;
    report["status"] = json!("registry_committed");
    report["generation"] = json!(generation);
    persist_handoff_receipt(&receipt_path, &report, true)?;

    finish_handoff_under_lease(
        &document,
        &target,
        &installed_stado,
        &receipt_path,
        report,
        None,
        json_output,
    )
    .await
}
