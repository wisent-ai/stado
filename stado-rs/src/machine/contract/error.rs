//! The structured failure every facade operation returns.

use crate::queue::StorageError;

/// Structured failure emitted by every facade operation. Serialized by the
/// CLI layer as `{"code","message","retryable"}`.
#[derive(Debug)]
pub struct MachineError {
    pub code: String,
    pub message: String,
    pub retryable: bool,
}

impl MachineError {
    pub fn new(code: &str, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            retryable: false,
        }
    }

    pub fn retryable(code: &str, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            retryable: true,
        }
    }
}

impl std::fmt::Display for MachineError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for MachineError {}

// Python `_invoke` maps any non-MachineError exception to INTERNAL with the
// stringified exception as the message; the From impls below reproduce that.
impl From<StorageError> for MachineError {
    fn from(exc: StorageError) -> Self {
        Self::new("INTERNAL", exc.to_string())
    }
}

impl From<std::io::Error> for MachineError {
    fn from(exc: std::io::Error) -> Self {
        Self::new("INTERNAL", exc.to_string())
    }
}

impl From<serde_json::Error> for MachineError {
    fn from(exc: serde_json::Error) -> Self {
        Self::new("INTERNAL", exc.to_string())
    }
}

/// A service-directory refusal keeps its own code instead of collapsing into
/// INTERNAL. SERVICE_DIRECTORY_STALE is the one a caller can act on without
/// a human: re-read the directory and call again, rather than treating a
/// handed-over service as an outage.
impl From<crate::targets::ServiceDirectoryError> for MachineError {
    fn from(exc: crate::targets::ServiceDirectoryError) -> Self {
        let (code, message) = (exc.code(), exc.to_string());
        if exc.retryable() {
            Self::retryable(code, message)
        } else {
            Self::new(code, message)
        }
    }
}
