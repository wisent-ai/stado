//! The Vast bridge error type, shared by the REST client and the auto-list
//! bridge.
//!
//! Moved verbatim out of the former single-file `providers/vast`.

use crate::queue::StorageError;

/// Vast bridge error. Python raises `VastConfigError` for config problems
/// and `RuntimeError` for HTTP failures; urllib/JSON exceptions propagate.
#[derive(Debug, thiserror::Error)]
pub enum VastError {
    /// Python `VastConfigError`.
    #[error("{0}")]
    Config(String),
    /// Python `RuntimeError` from `_request` (HTTP error status).
    #[error("{0}")]
    Api(String),
    /// Python urllib `URLError` etc. propagated from `_request`.
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

    /// The Python `type(exc).__name__` slot in "poll failed: {type}: {exc}".
    pub(super) fn kind(&self) -> &'static str {
        match self {
            VastError::Config(_) => "VastConfigError",
            VastError::Api(_) => "RuntimeError",
            VastError::Http(_) => "URLError",
            VastError::Storage(_) => "StorageError",
        }
    }
}
