//! The tick log sink, the error-section constructor, and the Python-shaped
//! value renderers every billing component formats through.

use serde_json::{json, Value};

pub(super) fn log(msg: &str) {
    eprintln!("[tick] {msg}");
}

pub(super) fn error_section(detail: String) -> Value {
    json!({"status": "error", "detail": detail})
}

/// Python `repr()` of a string and of a string list, as the billing sections
/// quote a dataset, a table or the keys a secret lacks: the one copy in
/// `models::python_compat`, which escapes a quote the value holds.
pub(super) use crate::models::{py_list_repr, py_str_repr as py_repr};

/// A billing section's value as it reads in an alert: text unquoted, numbers
/// in Python float style (serde_json prints 8.0 as "8.0"), and the shared
/// "not reported" where the section carries none. Python's `str()` printed
/// `None`, which read as a value the provider had answered.
pub(super) fn py_value(value: Option<&Value>) -> String {
    match value {
        None | Some(Value::Null) => crate::cli::reporting::NOT_REPORTED.to_string(),
        Some(Value::String(s)) => s.clone(),
        Some(other) => other.to_string(),
    }
}

/// Python `str()` of a float: integral floats keep a trailing ".0".
pub(super) fn py_f64(value: f64) -> String {
    if value.is_finite() && value == value.trunc() {
        format!("{value:.1}")
    } else {
        format!("{value}")
    }
}
