//! Python-shaped coercion of Box JSON values.
//!
//! Python `required_dict`, the truthiness and `str()` rules, and the
//! `str(...)` / `int(...)` / `bool(...)` field readers the `client` sibling
//! uses on every response dict.

use serde_json::{Map, Value};

use super::super::errors::BoxError;

/// Python `required_dict`.
pub fn required_dict(value: Value, context: &str) -> Result<Map<String, Value>, BoxError> {
    match value {
        Value::Object(map) => Ok(map),
        _ => Err(BoxError::transport(format!(
            "Box {context} response is not an object"
        ))),
    }
}

/// Python truthiness of the JSON value (None/False/0/""/[]/{} are falsy).
pub(crate) fn py_truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().is_some_and(|f| f != 0.0),
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}

/// Python `str(value)`: strings pass through, null becomes "None",
/// booleans become "True"/"False", containers their JSON form.
pub(crate) fn py_str(value: &Value) -> String {
    match value {
        Value::Null => "None".to_string(),
        Value::Bool(b) => if *b { "True" } else { "False" }.to_string(),
        Value::Number(n) => n.to_string(),
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// Python `str(value.get(key) or "")`: falsy values map to "".
pub(crate) fn jstr(value: Option<&Value>) -> String {
    match value {
        Some(v) if py_truthy(v) => py_str(v),
        _ => String::new(),
    }
}

/// Python `int(value.get(key) or "<default>")`: falsy -> default, floats
/// truncate, strings parse (Python `int()` strips whitespace).
pub(crate) fn jint_or(value: Option<&Value>, default: i64) -> Result<i64, BoxError> {
    let Some(value) = value.filter(|v| py_truthy(v)) else {
        return Ok(default);
    };
    match value {
        Value::Number(n) => Ok(n.as_f64().unwrap_or(0.0) as i64),
        Value::String(s) => s.trim().parse::<i64>().map_err(|_| {
            BoxError::value(format!(
                "invalid literal for int() with base 10: {:?}",
                s.trim()
            ))
        }),
        other => Err(BoxError::value(format!(
            "int() argument must be a string or a number, not {}",
            py_str(other)
        ))),
    }
}

/// Python `bool(value.get(key))`.
pub(crate) fn jbool(value: Option<&Value>) -> bool {
    value.is_some_and(py_truthy)
}

/// First truthy value rendered with [`py_str`], else `default_text` (Python
/// `v1 or v2 or "<default>"` chains in error-payload parsing).
pub(crate) fn first_truthy_str(values: &[Option<&Value>], default_text: &str) -> String {
    for value in values.iter().flatten() {
        if py_truthy(value) {
            return py_str(value);
        }
    }
    default_text.to_string()
}
