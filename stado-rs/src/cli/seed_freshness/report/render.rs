//! The report as an operator reads it on a terminal.

use serde_json::Value;

/// Render the report the way an operator reads it.
pub(in crate::cli::seed_freshness) fn render(report: &Value) -> String {
    let mut out = String::new();
    let target = report
        .get("target")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    let journal = report.get("evidence").and_then(|e| e.get("journal"));
    let records = journal
        .and_then(|j| j.get("sign_in_records"))
        .and_then(Value::as_i64)
        .unwrap_or(0);
    let runs = report
        .get("evidence")
        .and_then(|e| e.get("reauth_runs_seen"))
        .and_then(Value::as_i64)
        .unwrap_or(0);
    let rows = report
        .get("login_rows_read")
        .and_then(Value::as_i64)
        .unwrap_or(0);
    out.push_str(&format!(
        "{target}: {rows} login row(s) read, {records} recorded sign-in(s), {runs} reauth run(s) in the recording store\n"
    ));
    if let Some(detail) = report.get("vault_half_unavailable").and_then(Value::as_str) {
        out.push_str(&format!(
            "  the vault half is unavailable on this host: {detail}\n  \
             seed presence is unknown; what follows is the recorded sign-in \
             history only\n"
        ));
    }
    let empty = Vec::new();
    let findings = report
        .get("findings")
        .and_then(Value::as_array)
        .unwrap_or(&empty);
    if findings.is_empty() {
        out.push_str("  no login row carries or declares an authenticator seed\n");
        return out;
    }
    for finding in findings {
        let item = finding
            .get("login_item")
            .and_then(Value::as_str)
            .unwrap_or("?");
        let verdict = finding
            .get("verdict")
            .and_then(Value::as_str)
            .unwrap_or("?");
        // The counts belong on every row, not only on a rejection: they are how
        // a reader tells "no evidence" apart from "evidence that says nothing
        // about the seed".
        let recorded = finding
            .get("attempts_recorded")
            .and_then(Value::as_i64)
            .unwrap_or(0);
        let submitting = finding
            .get("code_submitting_attempts")
            .and_then(Value::as_i64)
            .unwrap_or(0);
        out.push_str(&format!(
            "  {verdict:<32} {item}\n    {recorded} recorded attempt(s), {submitting} of them submitted a code\n"
        ));
        if let Some(since) = finding.get("rejected_since").and_then(Value::as_str) {
            let attempts = finding
                .get("code_submitting_attempts")
                .and_then(Value::as_i64)
                .unwrap_or(0);
            out.push_str(&format!(
                "    every code submitted since {since} was refused ({attempts} attempt(s) submitted a code)\n"
            ));
        }
        if let Some(at) = finding.get("last_known_good_at").and_then(Value::as_str) {
            out.push_str(&format!("    a code from this seed was accepted at {at}\n"));
        }
        let markers = finding
            .get("markers")
            .and_then(Value::as_array)
            .map(|names| {
                names
                    .iter()
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>()
                    .join(", ")
            })
            .unwrap_or_default();
        if !markers.is_empty() {
            out.push_str(&format!("    markers: {markers}\n"));
        }
        if let Some(repair) = finding.get("repair").and_then(Value::as_str) {
            out.push_str(&format!("    repair: {repair}\n"));
        }
    }
    out
}
