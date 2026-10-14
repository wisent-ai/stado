//! The verbatim upload half of `registry push`: whose generation the swap
//! spends, and the read-back that proves what landed.

use crate::cli::registry::write::conflict::{
    RegistryActual, RegistryConflict, RegistryWriteError, REGISTRY_CONFLICT_EXIT,
};
use crate::cli::registry::write::document::confirm_written;
use crate::cli::registry::write::guards::refuse_unsafe_replace;
use crate::cli::CmdError;
use crate::queue::StorageError;
use crate::targets::RegistryStore;

/// A store failure during the upload, carrying the store's own class.
fn upload_failed(exc: StorageError) -> RegistryWriteError {
    let mut failed = CmdError::click(format!("registry upload failed: {exc}"));
    failed.failure = CmdError::from(exc).failure;
    RegistryWriteError::Failed(failed)
}

/// The read-back did not show what this upload wrote.
fn verification_failed(detail: &str) -> RegistryWriteError {
    RegistryWriteError::Failed(
        CmdError::click(detail).stating(crate::primitives::failure::FailureCode::InfraDown),
    )
}

/// The upload half of [`push`](crate::cli::registry::push): read the current generation, refuse the write
/// when it is not the one the caller made its edit against, refuse a write
/// that would delete a top-level key unless the operator said so,
/// compare-and-swap, then read back and confirm the generation holds the
/// document sent ([`confirm_written`]). Returns the new generation and the
/// one it replaced, `None` when this write created the registry.
///
/// `expected_generation` is the caller's own token, from an earlier
/// [`pull`](crate::cli::registry::pull). When it is `Some` the object must exist AND be at exactly that
/// generation, checked before any guard runs and before anything is written,
/// and the swap spends that token rather than the one this function just
/// read: a document edited against generation 9 cannot land on top of 10.
/// When it is `None` the swap is against the generation read here, which
/// only rules out a writer that lands between this read and this write.
///
/// `payload` is written verbatim, so [`push`](crate::cli::registry::push) still uploads the operator's
/// exact file bytes rather than a re-serialization of them.
///
/// `allow_empty_fleet` is deliberately NOT `--force`: see the floor below.
pub(in crate::cli::registry) async fn upload_payload(
    payload: &str,
    allow_removals: bool,
    allow_empty_fleet: bool,
    expected_generation: Option<&str>,
) -> Result<(String, Option<String>), RegistryWriteError> {
    let store = RegistryStore::open().await.map_err(upload_failed)?;
    let current = store.read_versioned().await.map_err(upload_failed)?;
    // Ahead of every guard and every write: a caller whose token no longer
    // names the canonical document is holding an edit to a document that no
    // longer exists, and the guards below cannot see that. They compare this
    // payload against whatever is there now, which is exactly the comparison
    // that passes while the write erases a publication the payload predates.
    if let Some(expected) = expected_generation {
        let actual = match current.as_ref() {
            Some(blob) if blob.version == expected => None,
            Some(blob) => Some(RegistryActual::Generation(blob.version.clone())),
            None => Some(RegistryActual::Absent),
        };
        if let Some(actual) = actual {
            return Err(RegistryWriteError::Conflict(RegistryConflict {
                location: store.location().to_string(),
                expected: expected.to_string(),
                actual,
            }));
        }
    }
    let previous_generation = current.as_ref().map(|blob| blob.version.clone());
    refuse_unsafe_replace(current.as_ref(), payload, allow_removals, allow_empty_fleet)
        .map_err(RegistryWriteError::Failed)?;
    let generation = match current {
        Some(blob) => {
            // The generation read here; when the caller brought a token the
            // check above proved it is this one, so the write is conditional
            // on the read the edit was made against.
            let token = blob.version.as_str();
            match store.compare_and_swap(token, payload).await {
                Ok(generation) => generation,
                // The document moved between this function's read and its
                // swap. Same answer as a stale `--if-generation`, because it
                // is the same lost update seen a few milliseconds later.
                Err(StorageError::StorageConflict(_)) => {
                    return Err(RegistryWriteError::Conflict(RegistryConflict {
                        location: store.location().to_string(),
                        expected: token.to_string(),
                        actual: RegistryActual::Raced,
                    }));
                }
                Err(exc) => return Err(upload_failed(exc)),
            }
        }
        None => {
            let created = store
                .create_if_absent(payload)
                .await
                .map_err(upload_failed)?;
            if !created {
                // Somebody created the object while this command was deciding
                // it was absent, so this write has no condition to stand on.
                return Err(RegistryWriteError::Failed(CmdError {
                    message: Some(format!(
                        "registry write refused: there was no document at {} when this \
                         command read it, and another writer created one before this write. \
                         Re-read the registry (`stado registry pull --with-generation`), \
                         re-apply the change to what it now says, and write again with the new \
                         token. Exit {REGISTRY_CONFLICT_EXIT} means exactly this.",
                        store.location()
                    )),
                    code: REGISTRY_CONFLICT_EXIT,
                    ..CmdError::default()
                }));
            }
            store
                .read_versioned()
                .await
                .map_err(upload_failed)?
                .ok_or_else(|| {
                    verification_failed("registry upload verification could not read the object")
                })?
                .version
        }
    };
    let sent: serde_json::Value = serde_json::from_str(payload).map_err(|error| {
        RegistryWriteError::Failed(
            CmdError::click(format!(
                "registry upload of generation {generation} cannot be confirmed: the payload \
                 it sent is not JSON: {error}"
            ))
            .stating(crate::primitives::failure::FailureCode::Config),
        )
    })?;
    confirm_written(&store, &generation, &sent)
        .await
        .map_err(RegistryWriteError::Failed)?;
    Ok((generation, previous_generation))
}
