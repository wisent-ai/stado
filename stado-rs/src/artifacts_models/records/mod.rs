//! The manifest member records: storage locations, producing-run
//! provenance, the stored verification stamp and the adapter run report.

mod location;
mod producer;
mod report;
mod verification;

pub use location::ArtifactLocation;
pub use producer::ArtifactProducer;
pub use report::VerificationReport;
pub use verification::ArtifactVerification;

use serde_json::{Map, Value};

use super::error::ArtifactError;

/// A field a record must carry as text: a JSON string, or refused naming
/// the record and the field. Python's `str()` turned a null into `None`, a
/// boolean into `True` and a number into its digits, and the manifest was
/// then written back as if it had said so.
pub(super) fn as_text(record: &str, key: &str, value: &Value) -> Result<String, ArtifactError> {
    match value {
        Value::String(text) => Ok(text.clone()),
        other => Err(ArtifactError::invalid_manifest(format!(
            "{record}.{key} must be text, not {other}"
        ))),
    }
}

/// A text field the record must name; a missing one is refused by name.
fn required_text(record: &str, map: &Map<String, Value>, key: &str) -> Result<String, ArtifactError> {
    match map.get(key) {
        Some(value) => as_text(record, key, value),
        None => Err(ArtifactError::invalid_manifest(format!("{record} names no {key}"))),
    }
}

/// A text field the writer may leave out: absent or null records nothing,
/// which `to_value` writes back as empty.
pub(super) fn recorded_text(record: &str, map: &Map<String, Value>, key: &str) -> Result<String, ArtifactError> {
    match map.get(key) {
        None | Some(Value::Null) => Ok(String::new()),
        Some(value) => as_text(record, key, value),
    }
}

/// A list of text the writer may leave out: absent or null is no entries.
/// A single value where a list belongs is refused, not wrapped into one.
fn text_list(record: &str, map: &Map<String, Value>, key: &str) -> Result<Vec<String>, ArtifactError> {
    match map.get(key) {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(Value::Array(items)) => items.iter().map(|item| as_text(record, key, item)).collect(),
        Some(other) => Err(ArtifactError::invalid_manifest(format!(
            "{record}.{key} must be a list of text, not {other}"
        ))),
    }
}
