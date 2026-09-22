//! `stado space report TARGET`: disk, memory, inventory, build caches and
//! both janitor states as one document.

mod accelerators;
mod lines;

use lines::{print_memory, print_volumes};

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

pub(super) fn watermark_json(target: &crate::targets::ComputeTarget, report: &Value) -> Value {
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

pub(super) async fn report(target_name: &str, json_output: bool) -> Result<(), CmdError> {
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
    // The account's own home, so a declared `~/` root is expanded to the path
    // the host's own `du` walked. Guessing `/Users` or `/home` from a platform
    // is what makes a coverage report name a directory that does not exist.
    let home = crate::deploy::host_channel::remote_home(&target, &runner)
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    let free_space = document.get("free_space").cloned().unwrap_or(Value::Null);
    let report = Value::Object(document);
    let declared_cleaners: Vec<super::coverage::DeclaredCleaner> = target
        .disk_cleanup
        .as_ref()
        .map(|policy| {
            policy
                .cleaners
                .iter()
                .map(|(name, cleaner)| super::coverage::DeclaredCleaner {
                    name: name.clone(),
                    root: cleaner.root.clone(),
                })
                .collect()
        })
        .unwrap_or_default();
    let coverage = super::coverage::section(
        &report,
        stages,
        &home,
        &target.release_platform,
        &free_space,
        &declared_cleaners,
        &target.name,
    );
    let mut document = report.as_object().cloned().unwrap_or_else(Map::new);
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
        super::coverage::print_coverage(&coverage, &free_space);
        println!("last pass: {last_pass}");
        print_memory(report.get("memory_reclaim").unwrap_or(&Value::Null));
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
    // A verdict that only ran out of seconds is reported, not fatal: every
    // other figure above it was already read, and the inventory's own budget
    // has behaved this way since the walk was given one. A host that refused
    // the read still fails the command.
    if let Some(error) = cache_report.error.filter(|error| !error.is_empty()) {
        if cache_report.timed_out {
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
