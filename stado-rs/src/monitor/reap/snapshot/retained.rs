//! Proof that a lifecycle object a decision promised to retain still carries
//! byte-identical content on the live destination.

use crate::monitor::reap::reads::required_snapshot_text;
use crate::queue::{JobStorage, StorageError};

pub(super) fn reconciliation_store_path(path: &str) -> Result<&str, StorageError> {
    path.strip_prefix("ecosystem/probierz/")
        .ok_or_else(|| StorageError::Other(format!("non-canonical lifecycle path {path}")))
}

pub(super) async fn prove_snapshot_content_retained(
    live: &JobStorage,
    snapshot: &JobStorage,
    path: &str,
) -> Result<String, StorageError> {
    let path = reconciliation_store_path(path)?;
    let expected = required_snapshot_text(snapshot, path).await?;
    let actual = required_snapshot_text(live, path).await?;
    if actual != expected {
        return Err(StorageError::Other(format!(
            "retained lifecycle content changed at {path}"
        )));
    }
    Ok(actual)
}
