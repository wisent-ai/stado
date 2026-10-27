//! Reading Boat's JSON values: the object check, text, booleans and the
//! first value an answer states among several places it may put one.

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

/// The text Boat answered as `field` in its `answer` answer. An absent or
/// null field says nothing and reads empty; any other value that is not a
/// JSON string is refused naming the answer, the field and what Boat sent.
/// Truthiness read `false` and `0` as absent and printed `true` as Python's
/// `True`, so a state, id or cursor of the wrong type passed as text.
pub(crate) fn jstr(value: Option<&Value>, field: &str, answer: &str) -> Result<String, BoxError> {
    match value {
        None | Some(Value::Null) => Ok(String::new()),
        Some(Value::String(text)) => Ok(text.clone()),
        Some(other) => Err(BoxError::transport(format!(
            "Boat's {answer} answer carries {field} as {other}, not text"
        ))),
    }
}

/// A value Boat sent, as an error message shows it: its JSON form, or that
/// the answer carried none.
pub(crate) fn shown(value: Option<&Value>) -> String {
    match value {
        Some(value) => value.to_string(),
        None => "nothing".to_string(),
    }
}

/// The boolean Boat answered as `field` in its `answer` answer. Only a JSON
/// boolean is one: truthiness read the string "false" as true and a missing
/// field as false, so a prompt that never finished could read as done and a
/// failed command as one that never said. Anything else is refused naming
/// the answer, the field and what Boat sent.
pub(crate) fn jbool(value: Option<&Value>, field: &str, answer: &str) -> Result<bool, BoxError> {
    match value {
        Some(Value::Bool(flag)) => Ok(*flag),
        Some(other) => Err(BoxError::transport(format!(
            "Boat's {answer} answer carries {field} as {other}, not a boolean"
        ))),
        None => Err(BoxError::transport(format!("Boat's {answer} answer carries no {field}"))),
    }
}

/// The whole number Boat answered as `field`, or `None` when the answer
/// carries none (absent or null). Any other value is refused naming the
/// answer and what Boat sent: read as `None`, an exit code Boat sent as text
/// or as a fraction read as a command that never said how it ended.
pub(crate) fn jint(value: Option<&Value>, field: &str, answer: &str) -> Result<Option<i64>, BoxError> {
    match value {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Number(number)) if number.is_i64() => Ok(number.as_i64()),
        Some(other) => Err(BoxError::transport(format!(
            "Boat's {answer} answer carries {field} as {other}, not a whole number"
        ))),
    }
}

/// The first value Boat stated among `values`, else `default_text`, the
/// sentence the caller states for an answer that named none of them. A value
/// is stated unless it is absent, null or empty text; text reads as itself
/// and any other value as its JSON form. Truthiness skipped `false` and `0`
/// as unstated and printed `true` as Python's `True`, so a code or reason
/// Boat sent could be replaced by the caller's sentence.
pub(crate) fn first_stated(values: &[Option<&Value>], default_text: &str) -> String {
    for value in values.iter().flatten() {
        match value {
            Value::Null => {}
            Value::String(text) if text.is_empty() => {}
            Value::String(text) => return text.clone(),
            other => return other.to_string(),
        }
    }
    default_text.to_string()
}
