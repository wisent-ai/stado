//! Artifact failure type and ref-segment validation.

use std::sync::LazyLock;

use regex::Regex;

/// Pattern every ref segment must satisfy: an ASCII letter or digit, then
/// letters, digits, `.`, `_` or `-`, at most 128 characters in all.
const SEGMENT_PATTERN: &str = "^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$";

static SEGMENT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(SEGMENT_PATTERN).expect("the segment pattern is a regex"));

fn is_segment(value: &str) -> bool {
    SEGMENT.is_match(value)
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

    /// The fleet failure class of this artifact code: a missing artifact or
    /// alias is not found, a conflict, an invalid reference or manifest, a
    /// failed verification and an actor that cannot be read are refusals of
    /// the request, and a corrupt stored record is the registry's own outage.
    /// A code this build does not know stays unknown rather than guessed.
    pub fn failure_code(&self) -> crate::primitives::failure::FailureCode {
        use crate::primitives::failure::FailureCode;
        match self.code.as_str() {
            "ARTIFACT_NOT_FOUND" => FailureCode::NotFound,
            "ARTIFACT_ALIAS_CONFLICT"
            | "ARTIFACT_VERSION_CONFLICT"
            | "ARTIFACT_INVALID_REF"
            | "ARTIFACT_INVALID_MANIFEST"
            | "ARTIFACT_VERIFICATION_FAILED"
            | "ARTIFACT_ACTOR_UNREADABLE" => FailureCode::Refused,
            "ARTIFACT_CORRUPT_ALIAS" | "ARTIFACT_CORRUPT_MANIFEST" => FailureCode::InfraDown,
            _ => FailureCode::Unknown,
        }
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
