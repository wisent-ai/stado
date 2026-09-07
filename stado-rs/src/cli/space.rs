use clap::Subcommand;
use serde_json::{json, Map, Value};

use super::{host, CmdError};

#[derive(Subcommand)]
pub enum SpaceCommands {
    /// Read disk, memory, inventory, build caches, and janitor state as one report.
    Report {
        target: String,
        #[arg(long)]
        json: bool,
    },
    /// Reclaim only fleet-declared stages, previewing unless --apply is present.
    Reclaim {
        target: String,
        /// Select a declared stage; repeat to select several. Omit for all stages.
        #[arg(long = "stage")]
        stages: Vec<String>,
        /// Report what the selected stages would remove and write no audit record.
        #[arg(long)]
        dry_run: bool,
        /// Remove what the selected stages name. Requires --reason.
        #[arg(long, conflicts_with = "dry_run")]
        apply: bool,
        /// Why the space is being reclaimed; recorded on the target beside its state.
        #[arg(long)]
        reason: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Perform one guarded single-file space operation.
    File {
        #[command(subcommand)]
        command: SpaceFileCommands,
    },
    /// Relocate object-store keys on the host that holds their bytes.
    Relocate {
        target: String,
        #[arg(long)]
        namespace: String,
        #[arg(long)]
        from_prefix: String,
        #[arg(long, default_value = "")]
        to_prefix: String,
        #[arg(long)]
        store_root: Option<String>,
        #[arg(long)]
        dry_run: bool,
        #[arg(long, conflicts_with = "dry_run")]
        apply: bool,
        #[arg(long, default_value_t = 0)]
        limit: usize,
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
pub enum SpaceFileCommands {
    /// Remove one regular file from a Stado-managed area of TARGET.
    Remove {
        target: String,
        path: String,
        #[arg(long)]
        json: bool,
    },
    /// Archive one obsolete executable or launchd declaration without deleting its bytes.
    Retire {
        target: String,
        path: String,
        #[arg(long)]
        product: String,
        #[arg(long)]
        dry_run: bool,
        #[arg(long)]
        transaction: Option<String>,
        #[arg(long)]
        expected_sha256: Option<String>,
        #[arg(long)]
        expected_size: Option<u64>,
        #[arg(long)]
        expected_mode: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Device-local primitive used by the target-resolving retire command.
    #[command(name = "retire-local", hide = true)]
    RetireLocal {
        path: String,
        #[arg(long)]
        product: String,
        #[arg(long)]
        dry_run: bool,
        #[arg(long)]
        transaction: Option<String>,
        #[arg(long)]
        expected_sha256: Option<String>,
        #[arg(long)]
        expected_size: Option<u64>,
        #[arg(long)]
        expected_mode: Option<String>,
        #[arg(long)]
        json: bool,
    },
}

pub async fn dispatch(command: SpaceCommands) -> Result<(), CmdError> {
    match command {
        SpaceCommands::Report { target, json } => report(&target, json).await,
        SpaceCommands::Reclaim {
            target,
            stages,
            dry_run: _,
            apply,
            reason,
            json,
        } => reclaim(&target, &stages, apply, reason.as_deref(), json).await,
        SpaceCommands::File { command } => match command {
            SpaceFileCommands::Remove { target, path, json } => {
                remove_file(&target, &path, json).await
            }
            SpaceFileCommands::Retire {
                target,
                path,
                product,
                dry_run,
                transaction,
                expected_sha256,
                expected_size,
                expected_mode,
                json,
            } => {
                retire_file(
                    &target,
                    host::RetireFileRequest {
                        path: &path,
                        product: &product,
                        dry_run,
                        transaction: transaction.as_deref(),
                        expected_sha256: expected_sha256.as_deref(),
                        expected_size,
                        expected_mode: expected_mode.as_deref(),
                    },
                    json,
                )
                .await
            }
            SpaceFileCommands::RetireLocal {
                path,
                product,
                dry_run,
                transaction,
                expected_sha256,
                expected_size,
                expected_mode,
                json,
            } => host::retire_file_local(
                host::RetireFileRequest {
                    path: &path,
                    product: &product,
                    dry_run,
                    transaction: transaction.as_deref(),
                    expected_sha256: expected_sha256.as_deref(),
                    expected_size,
                    expected_mode: expected_mode.as_deref(),
                },
                json,
            ),
        },
        SpaceCommands::Relocate {
            target,
            namespace,
            from_prefix,
            to_prefix,
            store_root,
            dry_run: _,
            apply,
            limit,
            json,
        } => {
            relocate(
                &target,
                &crate::deploy::host_object_relocate::RelocatePlan {
                    namespace,
                    from: from_prefix,
                    to: to_prefix,
                    store_root,
                    apply,
                    limit,
                },
                json,
            )
            .await
        }
    }
}

fn print_json(value: &Value) -> Result<(), CmdError> {
    println!("{}", serde_json::to_string_pretty(value)?);
    Ok(())
}

fn cache_json(
    declaration: &crate::deploy::host_build_caches::BuildCacheDeclaration,
    report: &crate::deploy::host_build_caches::BuildCacheReport,
) -> Value {
    json!({
        "declaration": {
            "source": "registry targets[].disk_cleanup.cleaners.build_caches",
            "root": declaration.root,
            "min_age_seconds": declaration.min_age_seconds,
        },
        "entries": report.entries.iter().map(|entry| json!({
            "verdict": entry.state,
            "path": entry.path,
            "kib": entry.kib,
        })).collect::<Vec<Value>>(),
        "error": report.error,
    })
}

fn watermark_json(target: &crate::targets::ComputeTarget, report: &Value) -> Value {
    let available_bytes = report
        .get("usage")
        .and_then(|usage| usage.get("available_kb"))
        .and_then(Value::as_str)
        .and_then(|value| value.parse::<i64>().ok())
        .and_then(|value| value.checked_mul(1024));
    let low_bytes = target
        .disk_cleanup
        .as_ref()
        .and_then(|policy| policy.low_free_gb.checked_mul(1024_i64.pow(3)));
    let target_bytes = target
        .disk_cleanup
        .as_ref()
        .and_then(|policy| policy.target_free_gb.checked_mul(1024_i64.pow(3)));
    json!({
        "available_bytes": available_bytes,
        "low_watermark_bytes": low_bytes,
        "target_watermark_bytes": target_bytes,
        "below_low_watermark": matches!((available_bytes, low_bytes), (Some(free), Some(low)) if free < low),
    })
}

async fn report(target_name: &str, json_output: bool) -> Result<(), CmdError> {
    let stages = crate::deploy::host_reclaim::declared_stages()
        .map_err(|error| CmdError::click(error.to_string()))?;
    let target = crate::deploy::host_channel::canonical_target(target_name)
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    let runner = crate::deploy::production_runner();
    let cache_declaration = crate::deploy::host_build_caches::declared_for_target(&target, &runner)
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    let (disk, cache_report) = tokio::join!(
        crate::deploy::host_disk::disk_target(&target, &runner),
        crate::deploy::host_build_caches::report_declaration_on_host(
            &target,
            &cache_declaration,
            &runner
        ),
    );
    let disk = disk.map_err(|error| CmdError::click(error.to_string()))?;
    let mut document = disk.as_object().cloned().unwrap_or_else(Map::new);
    document.insert("free_space".to_string(), watermark_json(&target, &disk));
    document.insert(
        "reclaim_stages".to_string(),
        Value::Array(
            stages
                .iter()
                .map(|stage| json!({"name": stage.name, "description": stage.description}))
                .collect(),
        ),
    );
    document.insert(
        "build_caches".to_string(),
        cache_json(&cache_declaration, &cache_report),
    );
    let report = Value::Object(document);
    if json_output {
        print_json(&report)?;
    } else {
        let usage = report.get("usage").unwrap_or(&Value::Null);
        let memory = report.get("memory").unwrap_or(&Value::Null);
        let state = report.get("cleanup_state").unwrap_or(&Value::Null);
        println!("{} space", target.name);
        println!(
            "disk: {} free KiB on {} ({})",
            usage
                .get("available_kb")
                .and_then(Value::as_str)
                .unwrap_or("unknown"),
            usage
                .get("filesystem")
                .and_then(Value::as_str)
                .unwrap_or("unknown filesystem"),
            usage
                .get("capacity")
                .and_then(Value::as_str)
                .unwrap_or("unknown capacity"),
        );
        println!(
            "memory: {} free KiB; swap {}",
            memory
                .get("free_kb")
                .and_then(Value::as_str)
                .unwrap_or("unknown"),
            memory
                .get("swap")
                .and_then(Value::as_str)
                .unwrap_or("unknown"),
        );
        println!(
            "janitor: {} (last pass {})",
            state
                .get("outcome")
                .and_then(Value::as_str)
                .unwrap_or("never_run"),
            state
                .get("last_pass_at")
                .and_then(Value::as_str)
                .unwrap_or("never"),
        );
        for entry in &cache_report.entries {
            println!("cache\t{}\t{}\t{}", entry.state, entry.kib, entry.path);
        }
    }
    if report.get("status").and_then(Value::as_str) != Some(crate::deploy::host_disk::OK_STATUS) {
        return Err(CmdError::click(format!(
            "{} space report could not read the host: {}",
            target.name,
            report
                .get("error")
                .and_then(Value::as_str)
                .unwrap_or("host read failed")
        ))
        .machine_readable(json_output));
    }
    if let Some(error) = cache_report.error.filter(|error| !error.is_empty()) {
        return Err(CmdError::click(format!(
            "{} build cache declaration could not be read: {error}",
            target.name
        ))
        .machine_readable(json_output));
    }
    Ok(())
}

async fn reclaim(
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
                &super::autonomy_cmd::actor(),
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

async fn remove_file(target: &str, path: &str, json_output: bool) -> Result<(), CmdError> {
    let outcome = host::remove_file_document(target, path).await?;
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

async fn retire_file(
    target: &str,
    request: host::RetireFileRequest<'_>,
    json_output: bool,
) -> Result<(), CmdError> {
    let outcome = host::retire_file_outcome(target, &request).await?;
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

async fn relocate(
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
