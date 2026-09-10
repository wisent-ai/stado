//! The space capability's mutating operations: reclaim, guarded single-file
//! removal and retirement, and object relocation.

use super::*;

pub(super) async fn reclaim(
    target_name: &str,
    requested: &[String],
    apply: bool,
    reason: Option<&str>,
    json_output: bool,
) -> Result<(), CmdError> {
    let selected = crate::deploy::host_reclaim::select_stages(requested)
        .map_err(|error| CmdError::usage(error.to_string()).machine_readable(json_output))?;
    let reason = reason.map(str::trim).filter(|text| !text.is_empty());
    if apply && reason.is_none() {
        return Err(CmdError::usage(
            "space reclaim --apply removes files and needs --reason <text>; the reason is appended to the target's own audit log beside the state it changed. Run without --apply to preview the declared stages",
        )
        .machine_readable(json_output));
    }
    let runner = crate::deploy::production_runner();
    let (target, reclamation) =
        crate::deploy::host_reclaim::reclaim_host(target_name, apply, &selected, &runner)
            .await
            .map_err(|error| CmdError::click(error.to_string()).machine_readable(json_output))?;
    let eligible = reclamation.stages.iter().any(|stage| {
        !stage
            .stage
            .ends_with(crate::deploy::host_reclaim::UNAVAILABLE_SUFFIX)
    });
    if !eligible {
        return Err(CmdError::click(format!(
            "{} declares no eligible space reclamation stage; add it to {} reclaim_stages",
            target.name,
            crate::deploy::host_reclaim::DECLARATION_PATH
        ))
        .machine_readable(json_output));
    }
    for (stage, detail) in &reclamation.skipped {
        crate::failure::log_failure(
            "cli.space.reclaim",
            "fleet",
            crate::failure::classify_message(detail),
            &format!("{stage}: {detail}"),
        );
    }
    let audit = match reason {
        Some(reason) if apply => Some(
            crate::deploy::host_reclaim::record_audit(
                &target,
                &reclamation,
                reason,
                &crate::cli::autonomy_cmd::actor(),
                &runner,
            )
            .await
            .map_err(|error| CmdError::click(error.to_string()).machine_readable(json_output))?,
        ),
        _ => None,
    };
    let mut document = crate::deploy::host_reclaim::to_report(&target, &reclamation);
    document.insert("selected_stages".to_string(), json!(selected));
    document.insert("audit_log".to_string(), json!(audit));
    let report = Value::Object(document);
    if json_output {
        print_json(&report)?;
    } else {
        println!(
            "{} — {} on {}",
            if apply {
                "APPLIED"
            } else {
                "DRY RUN; nothing was deleted"
            },
            reclamation.mode,
            target.name
        );
        for stage in &reclamation.stages {
            println!(
                "{}\t{} item(s)\t{:?} -> {:?}",
                stage.stage, stage.items, stage.free_kb_before, stage.free_kb_after
            );
            if let Some(detail) = &stage.detail {
                println!("  {detail}");
            }
        }
        if let Some(path) = audit.as_deref() {
            println!("audited: {path} on {}", target.name);
        }
    }
    Ok(())
}

pub(super) async fn remove_file(
    target: &str,
    path: &str,
    json_output: bool,
) -> Result<(), CmdError> {
    let outcome = host::remove_file_document(target, path)
        .await
        .map_err(|error| error.machine_readable(json_output))?;
    let report = json!({
        "target": outcome.target,
        "path": outcome.path,
        "status": outcome.status,
        "detail": outcome.detail,
    });
    if json_output {
        print_json(&report)
    } else {
        println!(
            "{}: {} {}{}",
            outcome.target,
            outcome.path,
            outcome.status,
            outcome
                .detail
                .as_deref()
                .map(|detail| format!(" — {detail}"))
                .unwrap_or_default(),
        );
        Ok(())
    }
}

pub(super) async fn retire_file(
    target: &str,
    request: host::RetireFileRequest<'_>,
    json_output: bool,
) -> Result<(), CmdError> {
    let outcome = host::retire_file_outcome(target, &request)
        .await
        .map_err(|error| error.machine_readable(json_output))?;
    if json_output {
        println!("{}", serde_json::to_string_pretty(&outcome)?);
    } else if outcome.status == "absent" {
        println!("{}: {} absent", outcome.target, outcome.source);
    } else {
        println!(
            "{}: {} {} -> {} (transaction {}, {} bytes, sha256 {}, mode {})",
            outcome.target,
            outcome.source,
            outcome.status,
            outcome.destination.as_deref().unwrap_or("-"),
            outcome.transaction.as_deref().unwrap_or("-"),
            outcome.size.unwrap_or(0),
            outcome.sha256.as_deref().unwrap_or("-"),
            outcome.mode.as_deref().unwrap_or("-"),
        );
    }
    Ok(())
}

pub(super) async fn relocate(
    target: &str,
    plan: &crate::deploy::host_object_relocate::RelocatePlan,
    json_output: bool,
) -> Result<(), CmdError> {
    let report = crate::deploy::host_object_relocate::relocate_host(
        target,
        plan,
        &crate::deploy::production_runner(),
    )
    .await
    .map_err(|error| CmdError::click(error.to_string()).machine_readable(json_output))?;
    if json_output {
        print_json(&report)?;
    } else {
        let totals = report.get("totals").unwrap_or(&Value::Null);
        println!(
            "{}: {} object(s) relocated; {} refused; complete={}",
            target,
            totals
                .get("moved")
                .and_then(Value::as_i64)
                .unwrap_or_default(),
            totals
                .get("refused")
                .and_then(Value::as_i64)
                .unwrap_or_default(),
            totals
                .get("complete")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        );
    }
    if report.get("status").and_then(Value::as_str)
        != Some(crate::deploy::host_object_relocate::OK_STATUS)
    {
        return Err(CmdError::click(format!(
            "{} object relocation failed: {}",
            target,
            report
                .get("error")
                .and_then(Value::as_str)
                .unwrap_or("host refused the relocation")
        ))
        .machine_readable(json_output));
    }
    Ok(())
}
