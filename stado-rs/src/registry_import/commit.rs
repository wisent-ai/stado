//! The compare-and-swap commit loop, and the one public entry point that
//! drives decode, merge, validate, commit and verify. A lost race is another
//! writer's commit landing; the merge is re-taken against it, with no round
//! count deciding when to give up.

use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::queue::StorageError;
use crate::targets::{self, RegistryStore};

use super::merge::{all_source_imported, merge_documents};
use super::receipt::{RegistryImportConflict, RegistryImportError, RegistryImportReceipt};
use super::source::{decode_source, source_rejection, validate_document};

async fn verify_write(
    store: &RegistryStore,
    expected_generation: &str,
    expected_content: &str,
) -> Result<(), RegistryImportError> {
    let confirmed = store
        .read_versioned()
        .await?
        .ok_or(RegistryImportError::Verification)?;
    if confirmed.version != expected_generation || confirmed.content != expected_content {
        return Err(RegistryImportError::Verification);
    }
    Ok(())
}

/// Import one complete registry-v2 document into the configured canonical
/// registry. Semantic conflicts and invalid inputs are receipts, not partial
/// failures; storage failures are operational errors.
pub async fn import_bytes(bytes: &[u8]) -> Result<RegistryImportReceipt, RegistryImportError> {
    let mut source = match decode_source(bytes) {
        Ok(source) => source,
        Err(reason) => return Ok(source_rejection(bytes, reason)),
    };
    targets::strip_retired_resource_declarations(&mut source);
    let source_sha256 = format!("{:x}", Sha256::digest(bytes));
    let store = RegistryStore::open().await?;

    loop {
        let current = store.read_versioned().await?;
        let Some(current) = current else {
            let payload = format!(
                "{}\n",
                serde_json::to_string_pretty(&source).map_err(|error| {
                    RegistryImportError::Storage(format!(
                        "cannot serialize source registry: {error}"
                    ))
                })?
            );
            if !store.create_if_absent(&payload).await? {
                continue;
            }
            let generation = store
                .read_versioned()
                .await?
                .ok_or(RegistryImportError::Verification)?
                .version;
            verify_write(&store, &generation, &payload).await?;
            let summary = all_source_imported(&source).map_err(RegistryImportError::Storage)?;
            return Ok(summary.into_receipt(
                source_sha256,
                "imported",
                Some(generation),
                Some("absent".to_string()),
            ));
        };
        let canonical: Value = serde_json::from_str(&current.content).map_err(|error| {
            RegistryImportError::CanonicalInvalid {
                generation: current.version.clone(),
                reason: format!("not valid JSON: {error}"),
            }
        })?;
        validate_document(&canonical).map_err(|reason| RegistryImportError::CanonicalInvalid {
            generation: current.version.clone(),
            reason,
        })?;

        let (mut candidate, mut summary) =
            merge_documents(&canonical, &source).map_err(RegistryImportError::Storage)?;
        targets::strip_retired_resource_declarations(&mut candidate);
        if !summary.conflicts.is_empty() {
            summary.discard_pending_imports();
            return Ok(summary.into_receipt(
                source_sha256,
                "conflict",
                Some(current.version),
                None,
            ));
        }
        if let Err(reason) = validate_document(&candidate) {
            summary.conflicts.push(RegistryImportConflict {
                path: "registry".to_string(),
                reason: format!("combining the two valid registries is invalid: {reason}"),
            });
            summary.discard_pending_imports();
            return Ok(summary.into_receipt(
                source_sha256,
                "conflict",
                Some(current.version),
                None,
            ));
        }
        if candidate == canonical {
            return Ok(summary.into_receipt(
                source_sha256,
                "unchanged",
                Some(current.version),
                None,
            ));
        }

        let payload = format!(
            "{}\n",
            serde_json::to_string_pretty(&candidate).map_err(|error| {
                RegistryImportError::Storage(format!("cannot serialize merged registry: {error}"))
            })?
        );
        match store.compare_and_swap(&current.version, &payload).await {
            Ok(generation) => {
                verify_write(&store, &generation, &payload).await?;
                return Ok(summary.into_receipt(
                    source_sha256,
                    "imported",
                    Some(generation),
                    Some(current.version),
                ));
            }
            Err(StorageError::StorageConflict(_)) => continue,
            Err(error) => return Err(error.into()),
        }
    }
}
