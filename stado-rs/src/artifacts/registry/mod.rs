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

/// Python `_actor()`: `f"{getpass.getuser()}@{socket.gethostname()}"`.
fn actor() -> String {
    let user = std::env::var("USER")
        .ok()
        .filter(|u| !u.is_empty())
        .or_else(|| std::env::var("LOGNAME").ok())
        .unwrap_or_default();
    let host = std::env::var("HOSTNAME")
        .ok()
        .filter(|h| !h.is_empty())
        .or_else(|| {
            std::process::Command::new("hostname")
                .output()
                .ok()
                .filter(|out| out.status.success())
                .map(|out| String::from_utf8_lossy(&out.stdout).trim().to_string())
                .filter(|h| !h.is_empty())
        })
        .unwrap_or_default();
    format!("{user}@{host}")
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

/// Python `str(value)` for JSON scalars (used where Python stringifies
/// `alias["target_version"]` etc.). Numbers render without a trailing
/// `.0`, matching Python's `str(int)` for integral values.
fn stringify_json_scalar(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Bool(b) => if *b { "True" } else { "False" }.to_string(),
        Value::Number(n) => n.to_string(),
        Value::Null => "None".to_string(),
        other => other.to_string(),
    }
}
