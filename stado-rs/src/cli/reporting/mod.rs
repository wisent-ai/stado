//! What the operator is shown: fleet status, one job's results, the alerts
//! that fired, and the table every command prints them in.

pub mod alerts;
pub mod results;
pub mod status;
pub mod table;

use serde_json::Value;

/// What a command shows for a report field the report does not carry: said
/// as absent, never as an empty word, a zero or `unknown`, each of which
/// reads as a value somebody measured.
pub const NOT_REPORTED: &str = "not reported";

/// A report field as a person reads it: text bare, any other value as the
/// JSON it is, and [`NOT_REPORTED`] for a field that is absent or null
/// (indexing a JSON object by a key it lacks answers null).
pub fn field(value: &Value) -> String {
    match value {
        Value::Null => NOT_REPORTED.to_string(),
        Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

/// A text field a reading carries as a word: the word, or [`NOT_REPORTED`]
/// for an empty one, which would print as nothing measured.
pub fn text(value: &str) -> String {
    if value.is_empty() {
        NOT_REPORTED.to_string()
    } else {
        value.to_string()
    }
}

/// [`text`] for a field the reading may not carry at all.
pub fn optional_text(value: Option<&str>) -> String {
    match value {
        Some(word) => text(word),
        None => NOT_REPORTED.to_string(),
    }
}
