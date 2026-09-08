//! The tick log sink, the error-section constructor, and the Python-shaped
//! value renderers every billing component formats through.

use serde_json::{json, Value};

pub(super) fn log(msg: &str) {
    eprintln!("[tick] {msg}");
}

pub(super) fn error_section(detail: String) -> Value {
    json!({"status": "error", "detail": detail})
}

/// Python `repr()` of a string: single quotes.
pub(super) fn py_repr(value: &str) -> String {
    format!("'{value}'")
}

/// Python `str()` of a string list: `['a', 'b']`.
pub(super) fn py_list_repr(items: &[&str]) -> String {
    let quoted: Vec<String> = items.iter().map(|i| format!("'{i}'")).collect();
    format!("[{}]", quoted.join(", "))
}

/// Python `str()` of a JSON value: strings unquoted, null -> "None",
/// numbers in Python float style (serde_json prints 8.0 as "8.0").
pub(super) fn py_value(value: Option<&Value>) -> String {
    match value {
        None | Some(Value::Null) => "None".to_string(),
        Some(Value::String(s)) => s.clone(),
        Some(other) => other.to_string(),
    }
}

/// Python `str()` of a float: integral floats keep a trailing ".0".
pub(super) fn py_f64(value: f64) -> String {
    if value.is_finite() && value.fract() == 0.0 {
        format!("{value:.1}")
    } else {
        format!("{value}")
    }
}
