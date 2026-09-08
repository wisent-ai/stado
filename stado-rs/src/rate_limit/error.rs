//! The one error type every rate-limit entry point answers with.

use crate::queue::StorageError;
use crate::skarbiec::SkarbiecError;

#[derive(Debug, thiserror::Error)]
pub enum RateLimitError {
    #[error("invalid rate-limit configuration: {0}")]
    Configuration(String),
    #[error("invalid rate-limit request: {0}")]
    InvalidRequest(String),
    #[error(transparent)]
    Storage(#[from] StorageError),
    #[error(transparent)]
    Skarbiec(#[from] SkarbiecError),
    #[error("invalid persisted rate-limit state: {0}")]
    State(String),
}
