//! Turning one host's reading into the report `stado space report` prints.

use super::*;

mod json;

pub use json::to_report;

/// Read the janitor's state document.
///
/// The keys are exactly the ones
/// [`crate::providers::local::disk_cleanup`]'s `write_state` emits:
/// `last_attempt_at` at the top level and the whole previous report under
/// `report`.
pub fn parse_state(payload: &str) -> CleanupState {
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
        error: None,
        report: report.cloned(),
    }
}

/// Read the complete space report inputs for an already-resolved target.
///
/// Two reads, deliberately. The cheap sections — usage, janitor state,
/// snapshots — are read first and always reported; the attribution walk then
/// runs until it ends, and when it fails the report carries its error instead
/// of the whole command failing.
pub async fn disk_target(target: &ComputeTarget, runner: &Runner) -> Result<Value, DeployError> {
    let gates = host_channel::run_script(
        target,
        &super::remote_script_for(super::DiskScope::GateInputs),
        runner,
    )
    .await?;
    let (output, attribution) =
        match host_channel::run_script(target, &remote_script(), runner).await {
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
    let reading = parse_output(&output.stdout);
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
