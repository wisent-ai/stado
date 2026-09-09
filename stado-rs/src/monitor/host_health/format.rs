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

/// Python `beacon.get(key) or '-'`.
fn py_or_dash(value: Option<&Value>) -> String {
    match value {
        Some(v) if py_truthy(v) => py_str(v),
        _ => "-".to_string(),
    }
}

/// Python `beacon.get(key, '-')` — the default applies only when the key is
/// absent, not when the value is falsy.
fn py_get_or_dash(value: Option<&Value>) -> String {
    value.map_or_else(|| "-".to_string(), py_str)
}

/// Render the health report for an operator without discarding raw logs
/// (Python `format_host_health`, line-for-line).
pub fn format_host_health(report: &HostHealthReport) -> String {
    let beacon = &report.beacon;
    let metadata = &report.object;
    let get = |key: &str| beacon.get(key);

    let mut lines = vec![
        format!("target: {}", py_or_dash(report.target.get("name"))),
        format!("host: {}", py_or_dash(get("host"))),
        format!("reported_at: {}", py_or_dash(get("reported_at"))),
        format!(
            "object_updated_at: {}",
            py_or_dash(metadata.get("updated_at"))
        ),
        format!(
            "object: {}#{}",
            py_str(metadata.get("uri").unwrap_or(&Value::Null)),
            py_str(metadata.get("generation").unwrap_or(&Value::Null)),
        ),
        format!(
            "disk: {}% used; {} GiB available",
            py_get_or_dash(get("disk_pct")),
            py_get_or_dash(get("disk_avail_gb")),
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
                    _ => "unknown".to_string(),
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
    lines.push(py_or_dash(get("last_log")));
    lines.join("\n")
}
