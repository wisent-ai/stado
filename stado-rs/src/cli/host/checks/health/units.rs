use serde_json::{json, Value};

use crate::cli::CmdError;
use crate::targets::ComputeTarget;

use crate::cli::host::checks::HOST_HEALTH_AUTH_UNAVAILABLE;
use crate::cli::host::secrets::vault::vault_word;

/// One collected managed-unit log, before the CLI or a higher-level
/// diagnostic chooses how to render it.
pub(in crate::cli::host) struct UnitLogReport {
    target: String,
    unit: String,
    lines: u32,
    declared: Vec<Value>,
    log: String,
}

impl UnitLogReport {
    fn to_json(&self) -> Value {
        json!({
            "target": self.target,
            "unit": self.unit,
            "lines": self.lines,
            "declared": self.declared,
            "log": self.log,
        })
    }
}

/// Collect a managed unit's logs through the service subsystem's shared
/// platform reader.
///
/// `service logs`, `host unit-log`, and higher-level diagnostics must resolve
/// launchd files and systemd scopes identically. The old implementation was a
/// second, Darwin-only reader: on Linux it searched three `Library`
/// directories and never reached the journal that held the failure.
pub(in crate::cli::host) async fn collect_unit_log(
    resolved: &ComputeTarget,
    unit: &str,
    lines: u32,
    runner: &crate::deploy::Runner,
) -> Result<UnitLogReport, CmdError> {
    let tail = crate::deploy::service::tail_unit_logs(resolved, unit, "", lines as usize, runner)
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;

    let source = |origin: &str| {
        let kind = if origin.starts_with("journalctl ") {
            "journal"
        } else {
            "file"
        };
        json!({"kind": kind, "path": origin})
    };
    let mut declared = vec![source(&tail.origin)];
    let mut log = tail.body;
    if let Some(error_origin) = tail.error_origin {
        declared.push(source(&error_origin));
        if !tail.error_body.is_empty() {
            if !log.is_empty() && !log.ends_with('\n') {
                log.push('\n');
            }
            log.push_str(&tail.error_body);
        }
    }

    Ok(UnitLogReport {
        target: resolved.name.clone(),
        unit: unit.to_string(),
        lines,
        declared,
        log: log.trim_end().to_string(),
    })
}

/// The tail of one managed unit's own log, through the same platform reader as
/// `stado service logs`.
///
/// A unit that crash-loops states why in its log and nowhere else: the health
/// beacon reports `failed` with an empty `last_log`, `service status` reports
/// the state, and `host exec` is a read-only allowlist that cannot read a file.
/// Without this the only route to the sentence naming the fault was an ssh
/// session, which is the one thing the fleet does not allow, so the fault got
/// guessed at instead.
pub async fn unit_log(
    target: &str,
    unit: &str,
    lines: Option<u32>,
    json: bool,
) -> Result<(), CmdError> {
    // The unit id becomes a fixed word in the shared launchd/systemd reader,
    // so reject anything that is not a single safe unit name first.
    vault_word("unit label", unit)?;
    let lines = lines.unwrap_or(40).clamp(u32::from(true), 200_000);
    let resolved = crate::deploy::host_channel::canonical_target(target)
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    let runner = crate::deploy::production_runner();
    let report = collect_unit_log(&resolved, unit, lines, &runner).await?;
    if json {
        println!("{}", serde_json::to_string_pretty(&report.to_json())?);
    } else {
        println!("{}", report.log);
    }
    Ok(())
}

/// The beacon publisher's newest outcome, read from the managed unit's own
/// declared log. A later successful host-name line supersedes an older error;
/// otherwise an incident that was already repaired would keep advertising the
/// old cause forever.
pub(in crate::cli::host) fn host_health_publisher_diagnosis(report: &UnitLogReport) -> Value {
    let lines = report.log.lines().collect::<Vec<_>>();
    let journal_success = format!(": {}", report.target);
    let last_success = lines.iter().rposition(|line| {
        let line = line.trim();
        line == report.target || line.ends_with(&journal_success)
    });
    let last_error = lines
        .iter()
        .rposition(|line| line.contains("Error:") || line.contains(HOST_HEALTH_AUTH_UNAVAILABLE));
    if matches!(
        (last_success, last_error),
        (Some(success), Some(error)) if success > error
    ) || matches!((last_success, last_error), (Some(_), None))
    {
        return json!({
            "unit": report.unit,
            "code": "published",
            "detail": "The beacon publisher's newest recorded attempt succeeded.",
            "repairable": false,
        });
    }
    if let Some(index) = last_error {
        if lines[index].contains(HOST_HEALTH_AUTH_UNAVAILABLE) {
            return json!({
                "unit": report.unit,
                "code": "verifier_unavailable",
                "detail": format!(
                    "The beacon publisher reached the host-health API, but that API could not read \
                     {}/token through its dedicated verifier.",
                    crate::config::HOST_HEALTH_API_ITEM
                ),
                "repairable": true,
                "repair_command": format!("stado repair stado --step link --target {} --apply", report.target),
            });
        }
        return json!({
            "unit": report.unit,
            "code": "publisher_failed",
            "detail": lines[index].trim(),
            "repairable": false,
        });
    }
    json!({
        "unit": report.unit,
        "code": "publisher_silent",
        "detail": "The beacon is stale, the host answers, and its publisher log names no failed publish.",
        "repairable": false,
    })
}
