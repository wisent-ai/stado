//! The report shape every command on this channel answers with: the target
//! and transport it opens with, the exit code and status it closes with, and
//! the last stderr line a failure carries verbatim.

use serde_json::{json, Map, Value};

use super::FAILED_STATUS;
use crate::deploy::CommandOutput;
use crate::targets::ComputeTarget;

/// The `target` / `ssh` head every report in this family opens with,
/// identical to the one `host reboot` emits.
pub fn base_report(target: &ComputeTarget) -> Map<String, Value> {
    let mut report = Map::new();
    report.insert("target".to_string(), json!(target.name));
    report.insert(
        "ssh".to_string(),
        target.ssh.as_ref().map_or(Value::Null, |ssh| json!(ssh)),
    );
    report.insert("ssh_fallbacks".to_string(), json!(target.ssh_fallbacks));
    report
}

/// The last line of the remote failure detail, verbatim.
///
/// Whatever actually went wrong — sudo asking for a password, a missing
/// binary, a refused key — the operator needs the remote's own words, not
/// a paraphrase. `fallback` covers a failure that produced no output at all.
pub fn last_error_line(output: &CommandOutput, fallback: &str) -> String {
    let detail = output.detail().trim();
    match detail.lines().next_back() {
        Some(line) => line.to_string(),
        None => fallback.to_string(),
    }
}

/// Close a report the way `host reboot` closes its own: `exit_code`, a
/// `status` string, and on failure the last stderr line under `error`.
pub fn finish_report(
    report: &mut Map<String, Value>,
    output: &CommandOutput,
    ok_status: &str,
    fallback_error: &str,
) {
    report.insert("exit_code".to_string(), json!(output.code));
    report.insert(
        "status".to_string(),
        json!(if output.ok() {
            ok_status
        } else {
            FAILED_STATUS
        }),
    );
    if !output.ok() {
        report.insert(
            "error".to_string(),
            json!(last_error_line(output, fallback_error)),
        );
    }
}

/// Split one marker line into its tab-separated fields.
///
/// Every remote script in this family speaks the tab-delimited `STADO_*`
/// marker protocol of [`crate::deploy::host_recovery::parse_output`]; the
/// parsers match on the resulting slice, so an unexpected field count
/// falls through instead of panicking on an index.
pub fn marker_fields(line: &str) -> Vec<&str> {
    line.split('\t').collect()
}
