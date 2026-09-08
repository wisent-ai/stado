//! The lease-layer error: which Python exception each variant carries and
//! how a lost race is told apart from an illegal transition.

use crate::queue::StorageError;

/// Lease-layer error. Python raises `LeaseConflict` (a `RuntimeError`
/// subclass) for lost races and invalid fences, `ValueError` for illegal
/// transitions / unsafe job ids / bad timestamps, and `RuntimeError` for
/// size and shape violations of the stored blob.
#[derive(Debug, thiserror::Error)]
pub enum LeaseError {
    /// Python `LeaseConflict`.
    #[error("{0}")]
    Conflict(String),
    /// Python `ValueError`.
    #[error("{0}")]
    Value(String),
    /// Python `RuntimeError` for a corrupt/oversized stored lease.
    #[error("{0}")]
    Corrupt(String),
    #[error(transparent)]
    Storage(#[from] StorageError),
}

impl LeaseError {
    pub(super) fn conflict(message: &str) -> Self {
        LeaseError::Conflict(message.to_string())
    }

    /// Whether this is a `LeaseConflict` (Python `except LeaseConflict`).
    pub fn is_conflict(&self) -> bool {
        matches!(self, LeaseError::Conflict(_))
    }
}
