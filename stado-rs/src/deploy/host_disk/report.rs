//! Turning one host's reading into the report `stado space report` prints.

use super::*;

/// Read the janitor's state document.
///
/// The keys are exactly the ones
/// [`crate::providers::local::disk_cleanup`]'s `write_state` emits:
/// `last_attempt_at` at the top level and the whole previous report under
/// `report`. `next_pass_at` is the interval gate in `run_with_lock` read
/// forwards — that gate compares `now - last_attempt_at` against the
/// registry policy's `check_interval_seconds`, so the next pass is the
/// sum of the two.
pub fn parse_state(payload: &str, policy_interval_seconds: Option<i64>) -> CleanupState {
    let document: Value = match serde_json::from_str(payload) {
        Ok(value) => value,
        Err(exc) => {
            return CleanupState {
                present: true,
                error: Some(exc.to_string()),
                ..CleanupState::default()
            };
        }
    };
    let report = document.get("report");
    let field = |key: &str| report.and_then(|value| value.get(key));
    let text = |key: &str| field(key).and_then(Value::as_str).map(str::to_string);
    let free_before = field("free_bytes_before").and_then(Value::as_i64);
    let free_after = field("free_bytes_after").and_then(Value::as_i64);
    let last_attempt = document.get("last_attempt_at").and_then(Value::as_f64);
    let next_pass_at = match (last_attempt, policy_interval_seconds) {
        (Some(attempt), Some(interval)) => {
            DateTime::from_timestamp(attempt.trunc() as i64, u32::default())
                .and_then(|stamp| stamp.checked_add_signed(TimeDelta::seconds(interval)))
                .map(crate::models::isoformat_utc)
        }
        _ => None,
    };
    CleanupState {
        present: true,
        path: None,
        last_pass_at: text("started_at").or_else(|| last_attempt.and_then(iso_from_epoch)),
        last_success_at: text("last_success_at"),
        last_prevented_at: document
            .get("last_prevented_at")
            .and_then(Value::as_f64)
            .and_then(iso_from_epoch),
        outcome: text("outcome"),
        writer: text("writer"),
        writer_version: text("writer_version"),
        writer_pid: field("writer_pid").and_then(Value::as_i64),
        free_bytes_before: free_before,
        free_bytes_after: free_after,
        freed_bytes: match (free_before, free_after) {
            (Some(before), Some(after)) => Some(after - before),
            _ => None,
        },
        next_pass_at,
        low_bytes: report.and_then(disk_cleanup::validated_report_low_bytes),
        error: None,
        report: report.cloned(),
    }
}

/// The reading as the `--json` report, in `host reboot`'s report shape.
pub fn to_report(target: &ComputeTarget, reading: &DiskReading) -> Map<String, Value> {
    let mut report = host_channel::base_report(target);
    report.insert(
        "usage".to_string(),
        reading.usage.as_ref().map_or(Value::Null, |usage| {
            json!({
                "filesystem": usage.filesystem,
                "blocks_kb": usage.blocks_kb,
                "used_kb": usage.used_kb,
                "available_kb": usage.available_kb,
                "capacity": usage.capacity,
                "mounted_on": usage.mounted_on,
            })
        }),
    );
    // Beside the disk, because a host that cannot allocate and a host that
    // cannot write fail in the same commands and are repaired differently.
    // `free_kb` and `swap` stay as they were spelled so a reader written
    // against the first version of this report keeps working; everything a
    // watermark is measured against is under `memory_reclaim`.
    report.insert(
        "memory".to_string(),
        json!({
            "free_kb": available_kb(&reading.memory),
            "swap": swap_line(&reading.memory),
        }),
    );
    report.insert("memory_reclaim".to_string(), memory_report(target, reading));
    // The registry policy verbatim — same struct the janitor resolves, so
    // the operator is reading the declaration the host actually obeys.
    report.insert(
        "policy".to_string(),
        target
            .disk_cleanup
            .as_ref()
            .and_then(|policy| serde_json::to_value(policy).ok())
            .unwrap_or(Value::Null),
    );
    let state = &reading.state;
    report.insert(
        "cleanup_state".to_string(),
        json!({
            "present": state.present,
            "path": state.path,
            "last_pass_at": state.last_pass_at,
            "last_success_at": state.last_success_at,
            "last_prevented_at": state.last_prevented_at,
            "outcome": state.outcome,
            "writer": state.writer,
            "writer_version": state.writer_version,
            "writer_pid": state.writer_pid,
            "free_bytes_before": state.free_bytes_before,
            "free_bytes_after": state.free_bytes_after,
            "freed_bytes": state.freed_bytes,
            "next_pass_at": state.next_pass_at,
            "low_bytes": state.low_bytes,
            "error": state.error,
            "report": state.report,
        }),
    );
    // The other half of every `lock_busy` and `cleanup_in_progress` an
    // operator has ever read: which process is holding the run lock.
    report.insert(
        "cleanup_lock".to_string(),
        json!({
            "read": reading.lock_read,
            "path": reading.lock_path,
            "held": !reading.lock_holders.is_empty(),
            "holders": reading
                .lock_holders
                .iter()
                .map(|holder| json!({"pid": holder.pid, "command": holder.command}))
                .collect::<Vec<Value>>(),
        }),
    );
    // Reported next to the usage it does not appear in: `size_bytes` is
    // deliberately absent rather than null, because macOS states no size and a
    // key an operator could read as "zero" is worse than a key that is not
    // there. `reclaimable_by_stado` is the finding.
    let snapshots = &reading.snapshots;
    report.insert(
        "local_snapshots".to_string(),
        json!({
            "supported": snapshots.supported,
            "count": snapshots.names.len(),
            "names": snapshots.names,
            "reclaimable_by_stado": snapshots.names.iter().any(|name| {
                name.starts_with("com.apple.TimeMachine.") && name.ends_with(".local")
            }),
        }),
    );
    report.insert(
        "inventory".to_string(),
        Value::Array(
            reading
                .inventory
                .iter()
                .map(|item| {
                    json!({
                        "path": item.path,
                        "bytes": item.blocks_kb.saturating_mul(1024),
                        "size_gb": gib_from_blocks(item.blocks_kb as f64),
                    })
                })
                .collect(),
        ),
    );
    report.insert("chromium_clone_root".to_string(), json!(reading.clone_root));
    report.insert(
        "chromium_clones".to_string(),
        Value::Array(
            reading
                .clone_summaries
                .iter()
                .map(|summary| {
                    json!({
                        "path": summary.path,
                        "total": summary.total,
                        "older_than_hour": summary.older_than_hour,
                        "older_than_day": summary.older_than_day,
                    })
                })
                .collect(),
        ),
    );
    report
}

/// The inventory traverses whole filesystems; it has an independent bound.
const INVENTORY_BUDGET: std::time::Duration = std::time::Duration::from_secs(900);
const INVENTORY_BUDGET_ENV: &str = "STADO_INVENTORY_BUDGET_SECONDS";

fn inventory_budget() -> Result<std::time::Duration, DeployError> {
    match std::env::var(INVENTORY_BUDGET_ENV) {
        Err(std::env::VarError::NotPresent) => Ok(INVENTORY_BUDGET),
        Ok(value) => value
            .parse::<u64>()
            .ok()
            .filter(|seconds| *seconds > 0)
            .map(std::time::Duration::from_secs)
            .ok_or_else(|| {
                DeployError(format!(
                    "{INVENTORY_BUDGET_ENV} must be a positive whole number of seconds"
                ))
            }),
        Err(error) => Err(DeployError(format!(
            "cannot read {INVENTORY_BUDGET_ENV}: {error}"
        ))),
    }
}

/// Read the complete space report inputs for an already-resolved target.
///
/// Two reads, deliberately. The cheap sections — usage, janitor state,
/// snapshots — cost under a second, and an operator must never lose them
/// because the attribution walk behind them is slow. They are read first on
/// the shared bound and always reported; the walk is then attempted on its
/// own budget, and when it does not finish the report says so instead of the
/// whole command failing.
pub async fn disk_target(target: &ComputeTarget, runner: &Runner) -> Result<Value, DeployError> {
    let budget = inventory_budget()?;
    let interval = target
        .disk_cleanup
        .as_ref()
        .map(|policy| policy.check_interval_seconds);
    let gates = host_channel::run_script(
        target,
        &super::remote_script_for(super::DiskScope::GateInputs),
        runner,
    )
    .await?;
    let full =
        host_channel::run_script_with_timeout(target, &remote_script(), budget, runner).await;
    let (output, attribution) = match full {
        Ok(output) if output.code == 0 => (output, None),
        Ok(output) => (
            gates,
            Some(format!(
                "inventory command exited {}: {}",
                output.code,
                output.stderr.trim()
            )),
        ),
        Err(error) => (gates, Some(format!("inventory read failed: {error}"))),
    };
    let reading = parse_output(&output.stdout, interval);
    let mut report = to_report(target, &reading);
    if let Some(detail) = attribution {
        report.insert("inventory_incomplete".to_string(), json!(detail));
    }
    host_channel::finish_report(&mut report, &output, OK_STATUS, "ssh failed");
    Ok(Value::Object(report))
}

/// Resolve a canonical registry target and read its complete space inputs.
pub async fn disk_host(target_name: &str, runner: &Runner) -> Result<Value, DeployError> {
    let target = host_channel::canonical_target(target_name).await?;
    disk_target(&target, runner).await
}
