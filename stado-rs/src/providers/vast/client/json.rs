//! The Python-shaped JSON readers and f-string renderers shared by the
//! machine-id lookup, the offer operations and the auto-list bridge. The
//! two renderers the bridge calls carry the wider visibility that reaches
//! it; the readers stay inside the client.
//!
//! Moved verbatim out of the former single-file `providers/vast`.

use serde_json::Value;

use crate::providers::vast::VastError;

/// Python `resp.get("machines") or resp.get("results") or []`: the first
/// truthy array wins (an empty "machines" list falls through to "results").
pub(super) fn machines_of(resp: &Value) -> Vec<&Value> {
    for key in ["machines", "results"] {
        if let Some(array) = resp.get(key).and_then(Value::as_array) {
            if !array.is_empty() {
                return array.iter().collect();
            }
        }
    }
    Vec::new()
}

/// Python `int(m["id"])`: numbers pass, numeric strings parse.
pub(super) fn machine_id_of(machine: &Value) -> Result<i64, VastError> {
    machine
        .get("id")
        .and_then(json_int)
        .ok_or_else(|| VastError::config(format!("Vast.ai machine lacks an int id: {machine}")))
}

/// Python `int(value)` for JSON scalars (strings included).
pub(super) fn json_int(value: &Value) -> Option<i64> {
    match value {
        Value::Number(n) => n.as_i64(),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
}

/// Python `str(value.get(key) or "")` for string-ish fields.
pub(super) fn jstr(value: Option<&Value>) -> String {
    match value {
        Some(Value::String(s)) => s.clone(),
        _ => String::new(),
    }
}

/// Python f-string rendering of a JSON scalar (None/True/False included).
pub(in crate::providers::vast) fn py_value_str(value: Option<&Value>) -> String {
    match value {
        None | Some(Value::Null) => "None".to_string(),
        Some(Value::Bool(b)) => if *b { "True" } else { "False" }.to_string(),
        Some(Value::Number(n)) => n.to_string(),
        Some(Value::String(s)) => s.clone(),
        Some(other) => other.to_string(),
    }
}

/// Python str(float): integral floats keep one decimal ("3600.0").
pub(in crate::providers::vast) fn py_float(value: f64) -> String {
    if value.is_finite() && value.fract() == 0.0 {
        format!("{value:.1}")
    } else {
        format!("{value}")
    }
}
