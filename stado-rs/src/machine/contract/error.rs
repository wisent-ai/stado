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

    /// The fleet failure class of this machine code. A request the machine
    /// API will not take (malformed, conflicting, not yet terminal, disabled
    /// provider, forbidden) is refused; a missing job or artifact is not
    /// found; a missing or rejected caller is authentication; a declared
    /// service without an endpoint is configuration; a request still in
    /// progress is ours to wait out like a rate limit; a failed upload,
    /// submit, cancel, hold or stale directory is an outage. INTERNAL wraps
    /// storage, I/O and JSON failures alike, so only its retryable form is
    /// classed (as an outage) and a code this build does not know stays
    /// unknown rather than guessed.
    pub fn failure_code(&self) -> crate::primitives::failure::FailureCode {
        use crate::primitives::failure::FailureCode;
        match self.code.as_str() {
            "NOT_FOUND" | "NO_ARTIFACTS" | "SERVICE_NOT_IN_DIRECTORY" => FailureCode::NotFound,
            "UNAUTHORIZED" => FailureCode::Auth,
            "FORBIDDEN"
            | "INVALID_REQUEST"
            | "INVALID_CURSOR"
            | "INVALID_SOURCE_ARCHIVE"
            | "IDEMPOTENCY_CONFLICT"
            | "NOT_TERMINAL"
            | "ARTIFACT_SECURITY"
            | "PROVIDER_DISABLED"
            | "PROVIDER_NOT_ENABLED" => FailureCode::Refused,
            "SERVICE_ENDPOINT_MISSING" => FailureCode::Config,
            "REQUEST_IN_PROGRESS" => FailureCode::RateLimit,
            "AUTH_UNAVAILABLE"
            | "SOURCE_UPLOAD_FAILED"
            | "SUBMIT_FAILED"
            | "CANCEL_FAILED"
            | "HOLD_UNAVAILABLE"
            | "HOLD_FAILED"
            | "SERVICE_DIRECTORY_STALE" => FailureCode::InfraDown,
            "INTERNAL" if self.retryable => FailureCode::InfraDown,
            _ => FailureCode::Unknown,
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
