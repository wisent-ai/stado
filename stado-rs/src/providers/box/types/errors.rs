//! The Box error enum and the structured, redacted API failure.
//!
//! Python `BoxConfigurationError`, `BoxTransportError` and the `ValueError`
//! argument-validation sites fold into [`BoxError`]; Python `BoxAPIError`
//! keeps its own [`BoxApiError`] payload so callers can match on the status
//! and on retryability.

use serde_json::{Map, Value};

use super::safe_text;

/// Box-layer error. Python raises `BoxConfigurationError` for local config
/// problems, `BoxTransportError` for bounded network/response failures,
/// `BoxAPIError` for structured redacted API failures, and `ValueError` for
/// client-side argument validation.
#[derive(Debug, thiserror::Error)]
pub enum BoxError {
    /// Python `BoxConfigurationError`.
    #[error("{0}")]
    Configuration(String),
    /// Python `BoxTransportError`.
    #[error("{0}")]
    Transport(String),
    /// Python `ValueError` from argument validation (invalid box id, empty
    /// command, out-of-bounds timeout, ...).
    #[error("{0}")]
    Value(String),
    /// Python `BoxAPIError`.
    #[error("{0}")]
    Api(#[from] BoxApiError),
}

impl BoxError {
    pub(crate) fn configuration(message: impl Into<String>) -> Self {
        BoxError::Configuration(message.into())
    }

    pub(crate) fn transport(message: impl Into<String>) -> Self {
        BoxError::Transport(message.into())
    }

    pub(crate) fn value(message: impl Into<String>) -> Self {
        BoxError::Value(message.into())
    }
}

/// Structured, redacted Box API failure (Python `BoxAPIError`). Every text
/// field passed through [`safe_text`] at construction, so a key or token
/// embedded by the server never reaches logs.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub struct BoxApiError {
    pub status: u16,
    pub code: String,
    pub message: String,
    pub request_id: String,
    pub retryable: bool,
}

impl BoxApiError {
    /// Python `BoxAPIError.__init__`.
    pub fn new(status: u16, code: &str, message: &str, request_id: &str, retryable: bool) -> Self {
        BoxApiError {
            status,
            code: safe_text(code, "box_error"),
            message: safe_text(message, "Box API request failed"),
            request_id: safe_text(request_id, ""),
            retryable,
        }
    }

    /// Python `BoxAPIError.to_record` (dict key order preserved).
    pub fn to_record(&self) -> Map<String, Value> {
        Map::from_iter([
            ("status".to_string(), Value::from(self.status)),
            ("code".to_string(), Value::from(self.code.clone())),
            ("message".to_string(), Value::from(self.message.clone())),
            (
                "request_id".to_string(),
                Value::from(self.request_id.clone()),
            ),
            ("retryable".to_string(), Value::from(self.retryable)),
        ])
    }
}

impl std::fmt::Display for BoxApiError {
    /// Python `BoxAPIError.__str__`.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let request_id = &self.request_id;
        let suffix = if request_id.is_empty() {
            String::new()
        } else {
            format!(" request_id={request_id}")
        };
        write!(
            f,
            "Box API HTTP {} [{}]: {}{}",
            self.status, self.code, self.message, suffix
        )
    }
}
