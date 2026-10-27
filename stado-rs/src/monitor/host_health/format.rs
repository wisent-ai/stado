//! The operator rendering of a loaded report: each beacon value as the
//! beacon stated it, and "not reported" where it stated nothing.

use serde_json::Value;

use super::HostHealthReport;

/// A value the beacon stated: text raw, everything else in its JSON
/// spelling (serde_json prints floats Python-style, e.g. "85.0").
fn shown(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// A beacon field as the beacon stated it, or that it did not report it.
/// Only an absent key, null or an empty string say nothing: `false`, zero and
/// an empty list are values the beacon held. Python truthiness printed a
/// beacon's `false` as not reported, so a stated "no" read as silence.
fn reported(value: Option<&Value>) -> String {
    match value {
        None | Some(Value::Null) => crate::cli::reporting::NOT_REPORTED.to_string(),
        Some(Value::String(text)) if text.is_empty() => crate::cli::reporting::NOT_REPORTED.to_string(),
        Some(v) => shown(v),
    }
}

/// A beacon measurement with its unit, or that the beacon carries no such
/// value; a value the beacon does carry is shown even when it is zero.
/// Python's `str()` printed a null measurement as `None` beside its unit.
fn measured(value: Option<&Value>, unit: &str) -> String {
    match value {
        None | Some(Value::Null) => crate::cli::reporting::NOT_REPORTED.to_string(),
        Some(v) => format!("{}{unit}", shown(v)),
    }
}

/// Render the health report for an operator without discarding raw logs
/// (Python `format_host_health`, line-for-line).
pub fn format_host_health(report: &HostHealthReport) -> String {
    let beacon = &report.beacon;
    let metadata = &report.object;
    let get = |key: &str| beacon.get(key);

    let mut lines = vec![
        format!("target: {}", reported(report.target.get("name"))),
        format!("host: {}", reported(get("host"))),
        format!("reported_at: {}", reported(get("reported_at"))),
        format!(
            "object_updated_at: {}",
            reported(metadata.get("updated_at"))
        ),
        format!(
            "object: {}#{}",
            reported(metadata.get("uri")),
            reported(metadata.get("generation")),
        ),
        format!(
            "disk: {} used; {} available",
            measured(get("disk_pct"), "%"),
            measured(get("disk_avail_gb"), " GiB"),
        ),
        format!("privacy: {}", privacy_line(get("privacy"))),
        format!(
            "scans: {}",
            match get("scans") {
                Some(scans) => crate::deploy::host_scan::record::line(scans),
                None => crate::cli::reporting::NOT_REPORTED.to_string(),
            }
        ),
        "units:".to_string(),
    ];

    if let Some(units) = get("units")
        .and_then(Value::as_object)
        .filter(|u| !u.is_empty())
    {
        for (name, state) in units {
            if let Some(state_map) = state.as_object() {
                let state_text = reported(state_map.get("state"));
                lines.push(format!("  {name}: {state_text}"));
            } else {
                lines.push(format!("  {name}: {}", reported(Some(state))));
            }
        }
    } else {
        lines.push(format!("  {}", crate::cli::reporting::NOT_REPORTED));
    }
    lines.push("last_log:".to_string());
    lines.push(reported(get("last_log")));
    lines.join("\n")
}

/// One line of what the beacon's publisher may read, folder by folder:
/// `Documents denied, Desktop granted, …`, or `not reported` for a beacon
/// with no measurement. `stado host privacy` prints the details and the remedy.
fn privacy_line(block: Option<&Value>) -> String {
    let Some(folders) = block
        .and_then(|block| block.get("folders"))
        .and_then(Value::as_object)
    else {
        return crate::cli::reporting::NOT_REPORTED.to_string();
    };
    folders
        .iter()
        .map(
            |(folder, entry)| match entry.get("state").and_then(Value::as_str) {
                Some(state) => format!("{folder} {state}"),
                None => format!("{folder} not reported"),
            },
        )
        .collect::<Vec<_>>()
        .join(", ")
}
