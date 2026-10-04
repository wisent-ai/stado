//! `stado space report TARGET`: disk, memory, inventory, build caches, the
//! disk-full rule's verdict and the janitor's last pass as one document.

mod accelerators;
mod lines;

use lines::print_volumes;

use super::*;

pub(super) fn print_json(value: &Value) -> Result<(), CmdError> {
    println!("{}", serde_json::to_string_pretty(value)?);
    Ok(())
}

pub(super) fn cache_json(
    declaration: &crate::deploy::host_build_caches::BuildCacheDeclaration,
    report: &crate::deploy::host_build_caches::BuildCacheReport,
) -> Value {
    json!({
        "scan": {
            "source": "the disk-full rule: the janitor's build_caches cleaner walks the whole home",
            "root": declaration.root,
        },
        "entries": report.entries.iter().map(|entry| json!({
            "verdict": entry.state,
            "path": entry.path,
            "kib": entry.kib,
        })).collect::<Vec<Value>>(),
        "error": report.error,
    })
}

pub(super) async fn report(target_name: &str, json_output: bool) -> Result<(), CmdError> {
    let stages = crate::deploy::host_reclaim::declared_stages().map_err(CmdError::from)?;
    let target = crate::cli::canonical_host(target_name).await?;
    let runner = crate::deploy::production_runner();
    let cache_declaration =
        crate::deploy::host_build_caches::home_scan_for_target(&target, &runner)
            .await
            .map_err(CmdError::from)?;
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
        "work_root".to_string(),
        target.work_root.clone().map_or(Value::Null, Value::String),
    );
    let mount = lines::work_root_mount(&Value::Object(document.clone()));
    document.insert("work_root_mount".to_string(), mount.map_or(Value::Null, Value::String));
    document.insert(
        "build_caches".to_string(),
        cache_json(&cache_declaration, &cache_report),
    );
    // The account's own home, so a declared `~/` root is expanded to the path
    // the host's own `du` walked. Guessing `/Users` or `/home` from a platform
    // is what makes a coverage report name a directory that does not exist.
    let home = crate::deploy::host_channel::remote_home(&target, &runner)
        .await
        .map_err(CmdError::from)?;
    let report = Value::Object(document);
    let weles_recordings_dir = target
        .weles
        .as_ref()
        .and_then(|weles| weles.recordings_dir.clone());
    let coverage = super::coverage::section(
        &report,
        stages,
        &home,
        &target.release_platform,
        weles_recordings_dir.as_deref(),
    );
    let mut document = report.as_object().cloned().unwrap_or_else(Map::new);
    document.insert("rule".to_string(), coverage["rule"].clone());
    document.insert("coverage".to_string(), coverage.clone());
    document.insert(
        "accelerators".to_string(),
        accelerators::accelerators_json(&target).await,
    );
    let report = Value::Object(document);
    if json_output {
        print_json(&report)?;
    } else {
        let usage = report.get("usage").unwrap_or(&Value::Null);
        let state = report.get("cleanup_state").unwrap_or(&Value::Null);
        let last_pass = state
            .get("last_pass_at")
            .and_then(Value::as_str)
            .unwrap_or("never");
        println!("{} space", target.name);
        if let Some(error) = report.get("inventory_incomplete").and_then(Value::as_str) {
            println!("inventory incomplete: {error}");
            println!("coverage lists only measured paths; missing paths have not been checked");
        }
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
        print_volumes(&report);
        super::coverage::print_coverage(&coverage);
        println!("last pass: {last_pass}");
        lines::print_memory(report.get("memory").unwrap_or(&Value::Null));
        if let Some(line) = report
            .get("accelerators")
            .and_then(|block| block.get("line"))
            .and_then(Value::as_str)
        {
            println!("accelerators: {line}");
        }
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
    // A verdict that ran out of seconds, or a walk the host's `find` ended
    // before it printed any tag, is reported as incomplete, not fatal: every
    // other figure above it was already read, and the host did answer. A
    // host that refused the read still fails the command.
    if let Some(error) = cache_report.error.filter(|error| !error.is_empty()) {
        let walk_failed = cache_report
            .entries
            .iter()
            .any(|entry| entry.state == crate::deploy::host_build_caches::SCAN_FAILED);
        if cache_report.timed_out || walk_failed {
            eprintln!("{} build cache verdict incomplete: {error}", target.name);
            return Ok(());
        }
        return Err(CmdError::click(format!(
            "{} build cache declaration could not be read: {error}",
            target.name
        ))
        .machine_readable(json_output));
    }
    Ok(())
}
