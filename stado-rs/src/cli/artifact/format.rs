//! Output rendering and argument parsing shared by the artifact verbs.
//!
//! The JSON helpers reproduce Python's `json.dumps` spellings exactly: the
//! `indent=2, sort_keys=True` form used almost everywhere, and the default
//! separator form used by `resolve` and `alias set`.

use serde_json::Value;

use crate::artifacts_models::ArtifactRef;

use crate::cli::CmdError;

/// Recursively sort object keys (Python `sort_keys=True`).
fn sorted(value: &Value) -> Value {
    match value {
        Value::Object(map) => {
            let btree: std::collections::BTreeMap<String, Value> = map
                .iter()
                .map(|(key, value)| (key.clone(), sorted(value)))
                .collect();
            Value::Object(btree.into_iter().collect())
        }
        Value::Array(items) => Value::Array(items.iter().map(sorted).collect()),
        other => other.clone(),
    }
}

/// Python `json.dumps(value, indent=2, sort_keys=True)` (ensure_ascii).
pub(super) fn json_pretty_sorted(value: &Value) -> String {
    let pretty =
        serde_json::to_string_pretty(&sorted(value)).expect("JSON serialization is infallible");
    crate::models::ensure_ascii(&pretty)
}

/// Python `json.dumps(value, sort_keys=True)` — default separators
/// (", " / ": "), ensure_ascii.
pub(super) fn json_sorted(value: &Value) -> String {
    crate::queue::python_json_dumps(&sorted(value)).expect("JSON serialization is infallible")
}

pub(super) fn parse_ref(value: &str) -> Result<ArtifactRef, CmdError> {
    Ok(ArtifactRef::parse(value)?)
}

/// Python `_artifact_labels`: KEY=VALUE pairs, preserving the raw strings.
pub(super) fn parse_labels(values: &[String]) -> Result<Vec<(String, String)>, CmdError> {
    let mut labels = Vec::new();
    for value in values {
        let Some((key, item)) = value.split_once('=') else {
            return Err(CmdError::click(format!(
                "label must be KEY=VALUE: '{value}'"
            )));
        };
        if key.is_empty() {
            return Err(CmdError::click("label key cannot be empty"));
        }
        labels.push((key.to_string(), item.to_string()));
    }
    Ok(labels)
}
