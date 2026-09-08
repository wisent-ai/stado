//! Artifact failure type, ref-segment validation and the `str()` shim the
//! tolerant `from_dict` paths share.

use serde_json::Value;

/// Pattern every ref segment must satisfy (Python `_SEGMENT`,
/// `^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$`), hand-rolled to avoid a regex
/// dependency:
/// 1-128 chars, first ASCII alphanumeric, rest also allowing
/// `.`, `_`, `-`.
const SEGMENT_PATTERN: &str = "^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$";

fn is_segment(value: &str) -> bool {
    let mut chars = value.chars();
    match chars.next() {
        Some(first) if first.is_ascii_alphanumeric() => {}
        _ => return false,
    }
    value.len() <= 128 && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

/// Stable, machine-readable artifact operation failure (Python
/// `ArtifactError`). `code` is the machine-readable half (`ARTIFACT_*`).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{message}")]
pub struct ArtifactError {
    pub code: String,
    pub message: String,
}

impl ArtifactError {
    pub fn new(code: &str, message: impl Into<String>) -> Self {
        Self {
            code: code.to_string(),
            message: message.into(),
        }
    }

    pub(super) fn invalid_ref(message: impl Into<String>) -> Self {
        Self::new("ARTIFACT_INVALID_REF", message)
    }

    pub(super) fn invalid_manifest(message: impl Into<String>) -> Self {
        Self::new("ARTIFACT_INVALID_MANIFEST", message)
    }
}

pub(super) fn segment(value: &str, label: &str) -> Result<String, ArtifactError> {
    if !is_segment(value) {
        return Err(ArtifactError::invalid_ref(format!(
            "{label} must match '{SEGMENT_PATTERN}'"
        )));
    }
    Ok(value.to_string())
}

/// `str()` semantics for JSON scalars, used by the tolerant `from_dict`
/// paths (Python stringifies whatever `dict.get` returns).
pub(super) fn py_str(value: &Value) -> String {
    match value {
        Value::Null => "None".to_string(),
        Value::Bool(b) => if *b { "True" } else { "False" }.to_string(),
        Value::Number(n) => n.to_string(),
        Value::String(s) => s.clone(),
        // Python would render list/dict reprs; that is pathological input
        // for these fields, so compact JSON is close enough.
        other => other.to_string(),
    }
}
