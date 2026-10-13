//! The operator rendering of a loaded report, plus the four Python
//! coercions it is written in terms of (`str`, truthiness, `x or '-'`,
//! `get(key, '-')`).

use serde_json::Value;

use super::HostHealthReport;

/// Python `str(value)`: strings raw, null -> "None", everything else in its
/// JSON spelling (serde_json prints floats Python-style, e.g. "85.0").
fn py_str(value: &Value) -> String {
    match value {
        Value::Null => "None".to_string(),
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// Python truthiness for the `x or '-'` branch.
fn py_truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64() != Some(0.0),
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}

/// A beacon field, or that the beacon did not report it. An empty or false
/// value counts as not reported, as the Python report's `or` did; a dash
/// read as a value the beacon held.
fn reported(value: Option<&Value>) -> String {
    match value {
        Some(v) if py_truthy(v) => py_str(v),
        _ => "not reported".to_string(),
    }
}

/// A beacon measurement with its unit, or that the beacon carries no such
/// key; a value the beacon does carry is shown even when it is zero.
fn measured(value: Option<&Value>, unit: &str) -> String {
    match value {
        Some(v) => format!("{}{unit}", py_str(v)),
        None => "not reported".to_string(),
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
            py_str(metadata.get("uri").unwrap_or(&Value::Null)),
            py_str(metadata.get("generation").unwrap_or(&Value::Null)),
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
                None => "not reported".to_string(),
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
                let state_value = state_map.get("state");
                let state_text = match state_value {
                    Some(v) if py_truthy(v) => py_str(v),
                    _ => "not reported".to_string(),
                };
                lines.push(format!("  {name}: {state_text}"));
            } else {
                lines.push(format!("  {name}: {}", py_str(state)));
            }
        }
    } else {
        lines.push("  -".to_string());
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
        return "not reported".to_string();
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
