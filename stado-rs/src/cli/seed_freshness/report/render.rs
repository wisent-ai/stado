//! The report as an operator reads it on a terminal.

use serde_json::Value;

/// Render the report the way an operator reads it.
pub(in crate::cli::seed_freshness) fn render(report: &Value) -> String {
    let mut out = String::new();
    // A count the report does not carry is not zero, and a report without a
    // target is not about a host named "unknown": each is said as missing.
    let count = |value: Option<&Value>, what: &str| match value.and_then(Value::as_i64) {
        Some(count) => format!("{count} {what}"),
        None => format!("{what} not reported"),
    };
    let target = match report.get("target").and_then(Value::as_str) {
        Some(target) => target.to_string(),
        None => "the report names no target".to_string(),
    };
    let evidence = report.get("evidence");
    let records = count(
        evidence
            .and_then(|e| e.get("journal"))
            .and_then(|j| j.get("sign_in_records")),
        "recorded sign-in(s)",
    );
    let runs = count(
        evidence.and_then(|e| e.get("reauth_runs_seen")),
        "reauth run(s) in the recording store",
    );
    let rows = count(report.get("login_rows_read"), "login row(s) read");
    out.push_str(&format!("{target}: {rows}, {records}, {runs}\n"));
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
