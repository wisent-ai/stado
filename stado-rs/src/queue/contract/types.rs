//! Descriptors and the error type the blob contract's signatures name.
//! Bodies lifted out of `queue/mod.rs` unchanged.

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};

/// Text blob content with its opaque backend version token for
/// compare-and-swap.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VersionedText {
    pub content: String,
    pub version: String,
}

/// Backend-agnostic blob descriptor used to filter on metadata before
/// downloading a body. Callers retain the backend separately.
#[derive(Debug, Clone)]
pub struct BlobInfo {
    pub name: String,
    pub updated: Option<DateTime<Utc>>,
    pub size: Option<u64>,
    pub metadata: BTreeMap<String, String>,
}

/// Storage-layer failures distinguish lost conditional-write races, missing
/// objects, invalid local paths, authentication, provider API, transport, and
/// serialization failures.
#[derive(Debug, thiserror::Error)]
pub enum StorageError {
    /// A conditional write lost a race with another writer.
    #[error("{0}")]
    StorageConflict(String),
    /// A conditional operation required a blob that does not exist.
    #[error("blob not found: {0}")]
    NotFound(String),
    /// A local backend path escaped the deployment root.
    #[error("storage path escapes deployment root: {0}")]
    PathEscape(String),
    /// GCS JSON API returned a non-success status other than 404/412.
    #[error("GCS API error HTTP {status}: {body}")]
    Gcs { status: u16, body: String },
    /// Stado object API returned a non-success status.
    #[error("Stado object API error HTTP {status}: {body}")]
    Stado { status: u16, body: String },
    /// Authentication could not be established for the configured store.
    ///
    /// Every backend that authenticates raises this: the Stado object API's
    /// token file, Azure's token exchange and GCS's application credentials.
    /// It said "GCP authentication failed" for all three, so a machine with no
    /// GCP configuration at all reported a refused Stado storage token file as
    /// a Google credential failure, and the operator reading it looked for a
    /// service account that was never involved.
    #[error("storage authentication failed: {0}")]
    Auth(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    /// Preserve the HTTP cause chain when callers display or persist this error.
    #[error("{}", crate::cli::http_failure(.0))]
    Http(#[from] reqwest::Error),
    #[error("{0}")]
    Other(String),
}
