//! The Vast bridge error type, shared by the REST client and the auto-list
//! bridge.
//!
//! Moved verbatim out of the former single-file `providers/vast`.

use crate::queue::StorageError;

/// Vast bridge error: a configuration problem, an answer Vast.ai gave (its
/// HTTP status when it refused, none when the answer was not JSON or could
/// not be read), a transport failure, or a queue-state read.
#[derive(Debug, thiserror::Error)]
pub enum VastError {
    /// Missing or malformed configuration: the key, the machine, a variable.
    #[error("{0}")]
    Config(String),
    /// Vast.ai answered, and the answer is not usable: `status` is the HTTP
    /// status of a refusal, unset for a body that could not be read or is
    /// not JSON.
    #[error("{detail}")]
    Api { status: Option<u16>, detail: String },
    /// Vast.ai could not be reached.
    #[error(transparent)]
    Http(#[from] reqwest::Error),
    /// Storage failures from the queue-state probes.
    #[error(transparent)]
    Storage(#[from] StorageError),
}

impl VastError {
    pub(super) fn config(message: impl Into<String>) -> Self {
        VastError::Config(message.into())
    }

    /// The name of the failure's kind in the bridge's "poll failed: {kind}:
    /// {error}" log line.
    pub(super) fn kind(&self) -> &'static str {
        match self {
            VastError::Config(_) => "VastConfigError",
            VastError::Api { .. } => "RuntimeError",
            VastError::Http(_) => "URLError",
            VastError::Storage(_) => "StorageError",
        }
    }

    /// The fleet failure class of a marketplace failure: configuration is
    /// config, an unreachable Vast.ai or one whose answer is unreadable is
    /// its outage (a timeout its own class), and a refusal is classed by its
    /// HTTP status, a 4xx the upstream table leaves unclassified being
    /// Vast.ai refusing the request. A queue-state failure is classed by
    /// the store (`From<StorageError> for CmdError`), so it is `None` here.
    pub fn failure_code(&self) -> Option<crate::primitives::failure::FailureCode> {
        use crate::primitives::failure::FailureCode;
        match self {
            VastError::Config(_) => Some(FailureCode::Config),
            VastError::Http(error) if error.is_timeout() => Some(FailureCode::Timeout),
            VastError::Http(_) | VastError::Api { status: None, .. } => {
                Some(FailureCode::InfraDown)
            }
            VastError::Api {
                status: Some(status),
                ..
            } => Some(match FailureCode::from_upstream_status(*status) {
                FailureCode::Unknown
                    if reqwest::StatusCode::from_u16(*status)
                        .is_ok_and(|code| code.is_client_error()) =>
                {
                    FailureCode::Refused
                }
                known => known,
            }),
            VastError::Storage(_) => None,
        }
    }
}
