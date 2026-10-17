//! Reading Boat's JSON values: the object check, truthiness and text.

use std::num::FpCategory;

use serde_json::{Map, Value};

use super::super::errors::BoxError;

/// `value` as an object, refused naming the answer it came in.
pub fn required_dict(value: Value, context: &str) -> Result<Map<String, Value>, BoxError> {
    match value {
        Value::Object(map) => Ok(map),
        other => Err(BoxError::transport(format!(
            "Boat's {context} answer is not an object: {other}"
        ))),
    }
}

/// Whether a JSON value counts as present: null, false, a zero, an empty
/// text, list or object do not.
pub(crate) fn py_truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().is_some_and(|f| f.classify() != FpCategory::Zero),
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}

/// A JSON value as text: strings pass through, null reads `None`, booleans
/// `True`/`False`, containers their JSON form.
pub(crate) fn py_str(value: &Value) -> String {
    match value {
        Value::Null => "None".to_string(),
        Value::Bool(b) => if *b { "True" } else { "False" }.to_string(),
        Value::Number(n) => n.to_string(),
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// The text of a present value, empty for one that is absent or does not
/// count as present.
pub(crate) fn jstr(value: Option<&Value>) -> String {
    match value {
        Some(v) if py_truthy(v) => py_str(v),
        _ => String::new(),
    }
}

/// Whether the value is present and counts as true.
pub(crate) fn jbool(value: Option<&Value>) -> bool {
    value.is_some_and(py_truthy)
}

/// The first present value as text, else `default_text`, the sentence the
/// caller states for an answer that named none of them.
pub(crate) fn first_truthy_str(values: &[Option<&Value>], default_text: &str) -> String {
    for value in values.iter().flatten() {
        if py_truthy(value) {
            return py_str(value);
        }
    }
    default_text.to_string()
}
