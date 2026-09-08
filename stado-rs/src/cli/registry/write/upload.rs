//! The verbatim upload half of `registry push`: whose generation the swap
//! spends, and the read-back that proves what landed.

use crate::cli::registry::write::conflict::{RegistryActual, RegistryConflict, RegistryWriteError};
use crate::cli::registry::write::guards::refuse_unsafe_replace;
use crate::cli::CmdError;
use crate::queue::StorageError;
use crate::targets::RegistryStore;

/// The upload half of [`push`](crate::cli::registry::push): read the current generation, refuse the write
/// when it is not the one the caller made its edit against, refuse a write
/// that would delete a top-level key unless the operator said so,
/// compare-and-swap, then read back and verify BOTH the generation and the
/// bytes. Returns `(generation, previous_generation)`.
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
) -> Result<(String, String), RegistryWriteError> {
    let store = RegistryStore::open().await.map_err(|exc| {
        RegistryWriteError::Failed(CmdError::click(format!("registry upload failed: {exc}")))
    })?;
    let current = store.read_versioned().await.map_err(|exc| {
        RegistryWriteError::Failed(CmdError::click(format!("registry upload failed: {exc}")))
    })?;
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
    let previous_generation = current
        .as_ref()
        .map(|blob| blob.version.clone())
        .unwrap_or_else(|| "0".to_string());
    refuse_unsafe_replace(current.as_ref(), payload, allow_removals, allow_empty_fleet)
        .map_err(RegistryWriteError::Failed)?;
    let generation = match current {
        Some(blob) => {
            // The caller's token when it brought one, this read's generation
            // otherwise. They are equal here — the check above proved it — and
            // spending the caller's own token is what makes the write
            // conditional on the read the edit was made against.
            let token = expected_generation.unwrap_or(blob.version.as_str());
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
                Err(exc) => {
                    return Err(RegistryWriteError::Failed(CmdError::click(format!(
                        "registry upload failed: {exc}"
                    ))));
                }
            }
        }
        None => {
            let created = store.create_if_absent(payload).await.map_err(|exc| {
                RegistryWriteError::Failed(CmdError::click(format!(
                    "registry upload failed: {exc}"
                )))
            })?;
            if !created {
                // Somebody created the object while this command was deciding
                // it was absent, so this write has no condition to stand on.
                return Err(RegistryWriteError::Conflict(RegistryConflict {
                    location: store.location().to_string(),
                    expected: expected_generation.unwrap_or("0").to_string(),
                    actual: RegistryActual::Raced,
                }));
            }
            store
                .read_versioned()
                .await
                .map_err(|exc| {
                    RegistryWriteError::Failed(CmdError::click(format!(
                        "registry upload failed: {exc}"
                    )))
                })?
                .ok_or_else(|| {
                    RegistryWriteError::Failed(CmdError::click(
                        "registry upload verification could not read the object",
                    ))
                })?
                .version
        }
    };
    let confirmed = store
        .read_versioned()
        .await
        .map_err(|exc| {
            RegistryWriteError::Failed(CmdError::click(format!("registry upload failed: {exc}")))
        })?
        .ok_or_else(|| {
            RegistryWriteError::Failed(CmdError::click(
                "registry upload verification could not read the object",
            ))
        })?;
    if confirmed.version != generation || confirmed.content != payload {
        return Err(RegistryWriteError::Failed(CmdError::click(
            "registry upload verification returned different bytes",
        )));
    }
    Ok((generation, previous_generation))
}
