//! Immutable manifests and atomic mutable aliases over [`JobStorage`].
//!
//! Port of `stado/artifacts/registry.py`. Layout on the blob backend:
//! manifests are immutable documents at
//! `artifacts/manifests/<type>/<namespace>/<name>/<version>.json` (create-
//! if-absent; re-publish of identical content is idempotent, changed
//! content is `ARTIFACT_VERSION_CONFLICT`), aliases are mutable one-file
//! records at `artifacts/aliases/...` updated through compare-and-swap.

use serde_json::Value;

use crate::artifacts_models::{ArtifactError, ArtifactRef};
use crate::queue::{JobStorage, StorageError};

mod alias;
mod lookup;
mod publish;
mod verify;

const MANIFEST_PREFIX: &str = "artifacts/manifests";
const ALIAS_PREFIX: &str = "artifacts/aliases";

fn manifest_path(reference: &ArtifactRef) -> String {
    format!(
        "{MANIFEST_PREFIX}/{}/{}/{}/{}.json",
        reference.r#type, reference.namespace, reference.name, reference.version
    )
}

fn alias_path(reference: &ArtifactRef) -> String {
    format!(
        "{ALIAS_PREFIX}/{}/{}/{}/{}.json",
        reference.r#type, reference.namespace, reference.name, reference.version
    )
}

/// Python `_now()`: `datetime.now(timezone.utc).isoformat()`.
fn now() -> String {
    crate::models::isoformat_utc(chrono::Utc::now())
}

/// Who publishes or moves an alias: `<account>@<host>`, the account read as a
/// job submitter is. Refused when it cannot be read: read as empty, the
/// record named `@<host>`, an actor with no account.
fn actor() -> Result<String, RegistryError> {
    let user = crate::queue::submit::submitter()
        .map_err(|error| ArtifactError::new("ARTIFACT_ACTOR_UNREADABLE", error.to_string()))?;
    let host = crate::providers::vast::system_hostname();
    Ok(format!("{user}@{host}"))
}

/// Registry operation failure. [`RegistryError::Artifact`] is the
/// machine-readable Python `ArtifactError`; [`RegistryError::Storage`] is
/// an underlying blob-backend failure (Python lets those propagate).
#[derive(Debug, thiserror::Error)]
pub enum RegistryError {
    #[error("{0}")]
    Artifact(#[from] ArtifactError),
    #[error("{0}")]
    Storage(#[from] StorageError),
}

/// The artifact registry facade (Python `ArtifactRegistry`). Cheap to
/// clone — it only holds the [`JobStorage`] handle.
#[derive(Clone)]
pub struct ArtifactRegistry {
    store: JobStorage,
}

impl ArtifactRegistry {
    /// Registry over the configured storage (Python
    /// `ArtifactRegistry()` → `JobStorage(BUCKET)`).
    pub async fn new() -> Result<Self, StorageError> {
        Ok(Self {
            store: JobStorage::new().await?,
        })
    }

    /// Registry over an explicit store (tests, custom deployments).
    pub fn with_store(store: JobStorage) -> Self {
        Self { store }
    }
}

/// An alias record's text field: its JSON string, or `None` when the record
/// carries none or carries another type, which every caller refuses as a
/// corrupt alias naming the field. Python's `str()` turned a null target
/// into the version `None`, and the lookup then reported that version as
/// missing instead of the record as corrupt.
fn alias_text(value: &Value) -> Option<String> {
    value.as_str().map(str::to_string)
}
