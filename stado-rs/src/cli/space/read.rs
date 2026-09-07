//! `stado space report TARGET`: disk, memory, inventory, build caches and
//! both janitor states as one document.

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
    let report = Value::Object(document);
    if json_output {
        print_json(&report)?;
    } else {
        let usage = report.get("usage").unwrap_or(&Value::Null);
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
        print_memory(report.get("memory_reclaim").unwrap_or(&Value::Null));
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

/// The memory lines of the human-readable report.
///
/// Three facts, in the order an operator needs them: what the host has, what
/// it is measured against and whether it is over, and what the last pass
/// actually did. A number with no watermark beside it is what this report
/// printed before 2026-09-06, and it is why a host that could not give a
/// runtime its heap read as healthy.
pub(super) fn print_memory(memory: &Value) {
    let policy = memory
        .get("declaration")
        .and_then(|declaration| declaration.get("policy"))
        .unwrap_or(&Value::Null);
    let number = |value: &Value, key: &str| -> String {
        value
            .get(key)
            .and_then(Value::as_i64)
            .map_or_else(|| "unknown".to_string(), |found| found.to_string())
    };
    let reading = memory.get("reading").unwrap_or(&Value::Null);
    let mib = crate::providers::local::host_memory::constants::MIB;
    let total_mb = reading
        .get("total_bytes")
        .and_then(Value::as_i64)
        .map_or_else(|| "unknown".to_string(), |bytes| (bytes / mib).to_string());
    println!(
        "memory: {} MiB available of {total_mb} MiB, swap {}% used",
        number(reading, "available_mb"),
        number(reading, "swap_used_pct"),
    );
    println!(
        "memory watermark: mode {}, low {} MiB, target {} MiB, swap {}%{}",
        policy
            .get("mode")
            .and_then(Value::as_str)
            .unwrap_or("unknown"),
        number(policy, "low_free_mb"),
        number(policy, "target_free_mb"),
        number(policy, "high_swap_used_pct"),
        if memory
            .get("declaration")
            .and_then(|declaration| declaration.get("declared"))
            .and_then(Value::as_bool)
            == Some(true)
        {
            ""
        } else {
            " (reporting default: this host declares none)"
        }
    );
    let last = memory.get("last_pass").unwrap_or(&Value::Null);
    let report = last.get("report").unwrap_or(&Value::Null);
    println!(
        "memory janitor: {} (writer {}, at {})",
        report
            .get("outcome")
            .and_then(Value::as_str)
            .unwrap_or("never_run"),
        report
            .get("writer")
            .and_then(Value::as_str)
            .unwrap_or("none"),
        report
            .get("started_at")
            .and_then(Value::as_str)
            .unwrap_or("never"),
    );
    if report.get("refuse_placement").and_then(Value::as_bool) == Some(true)
        && report.get("pressure_active").and_then(Value::as_bool) == Some(true)
    {
        println!(
            "memory placement: refused — this host publishes accepting_jobs=false with \
             admission_reason memory_pressure_active while it is over its watermark"
        );
    }
}
