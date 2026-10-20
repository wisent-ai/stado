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
    // Evidence the host could not read is said beside the counts: a count
    // over a journal or a recording store with unreadable parts is a lower
    // bound, not the whole history.
    let journal = evidence.and_then(|e| e.get("journal"));
    if let Some(lines) = journal
        .and_then(|j| j.get("unreadable_lines"))
        .and_then(Value::as_array)
        .filter(|lines| !lines.is_empty())
    {
        for line in lines {
            out.push_str(&format!("  journal record not read: {line}\n"));
        }
    }
    if let Some(unattributed) = journal
        .and_then(|j| j.get("unattributed_records"))
        .and_then(Value::as_i64)
        .filter(|count| count.is_positive())
    {
        out.push_str(&format!(
            "  {unattributed} recorded sign-in(s) name no login item and are left out\n"
        ));
    }
    if let Some(roots) = evidence
        .and_then(|e| e.get("unreadable_recordings"))
        .and_then(Value::as_array)
        .filter(|roots| !roots.is_empty())
    {
        for root in roots {
            out.push_str(&format!("  recording store not read: {root}\n"));
        }
    }
    if let Some(detail) = report.get("vault_half_unavailable").and_then(Value::as_str) {
        out.push_str(&format!(
            "  the vault half is unavailable on this host: {detail}\n  \
             seed presence is unknown; what follows is the recorded sign-in \
             history only\n"
        ));
    }
    let Some(findings) = report.get("findings").and_then(Value::as_array) else {
        out.push_str("  the report carries no findings list\n");
        return out;
    };
    if findings.is_empty() {
        out.push_str("  no login row carries or declares an authenticator seed\n");
        return out;
    }
    for finding in findings {
        // A finding field the report left out is named as such, never as a
        // count of zero nobody measured.
        let field = |name: &str| match finding.get(name).and_then(Value::as_str) {
            Some(text) => text.to_string(),
            None => crate::cli::reporting::NOT_REPORTED.to_string(),
        };
        let count = |name: &str| match finding.get(name).and_then(Value::as_i64) {
            Some(count) => count.to_string(),
            None => crate::cli::reporting::NOT_REPORTED.to_string(),
        };
        let item = field("login_item");
        let verdict = field("verdict");
        let recorded = count("attempts_recorded");
        let submitting = count("code_submitting_attempts");
        out.push_str(&format!(
            "  {verdict:<32} {item}\n    {recorded} recorded attempt(s), {submitting} of them submitted a code\n"
        ));
        if let Some(since) = finding.get("rejected_since").and_then(Value::as_str) {
            let attempts = count("code_submitting_attempts");
            out.push_str(&format!(
                "    every code submitted since {since} was refused ({attempts} attempt(s) submitted a code)\n"
            ));
        }
        if let Some(at) = finding.get("last_known_good_at").and_then(Value::as_str) {
            out.push_str(&format!("    a code from this seed was accepted at {at}\n"));
        }
        match finding.get("markers").and_then(Value::as_array) {
            Some(names) if names.is_empty() => {}
            Some(names) => {
                let names: Vec<String> = names
                    .iter()
                    .map(|name| match name.as_str() {
                        Some(name) => name.to_string(),
                        None => format!("(not a marker name: {name})"),
                    })
                    .collect();
                out.push_str(&format!("    markers: {}\n", names.join(", ")));
            }
            None => out.push_str("    markers: not reported\n"),
        }
        if let Some(repair) = finding.get("repair").and_then(Value::as_str) {
            out.push_str(&format!("    repair: {repair}\n"));
        }
    }
    out
}
