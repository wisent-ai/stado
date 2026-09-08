//! The programmatic read-modify-write path: one versioned read, one
//! validated compare-and-swap, and the bounded retry a pure transform earns.

use serde_json::Value;

use crate::cli::registry::write::conflict::{
    RegistryActual, RegistryConflict, REGISTRY_CONFLICT_EXIT,
};
use crate::cli::CmdError;
use crate::queue::StorageError;
use crate::targets::{self, RegistryStore};

/// How many times [`commit_document`] re-reads and re-applies a pure
/// transform before it hands the conflict back.
///
/// Bounded because an unbounded retry against a document some other loop is
/// rewriting every second is a command that never returns. Sixteen rounds
/// outlast every burst this fleet has produced; past that the contention is
/// the thing to report, not to sit inside.
const COMMIT_ROUNDS: usize = 16;

/// Read the canonical document, apply a pure transform to it, and write the
/// result back conditionally on the generation that read produced — retrying
/// the whole round when somebody else wrote first.
///
/// This is the correct loop for exactly one shape of caller: one whose
/// `transform` is a function of the document and nothing else. Re-running such
/// a transform against a newer document is the whole point, because the answer
/// it produces is the answer for THAT document. A caller that has already
/// installed a key, stopped a unit or probed a host between its read and its
/// write must NOT be here: re-applying its transform would republish a
/// decision taken against state that has since changed. Those callers take a
/// single conditional attempt from their own [`fetch_versioned_document`] and
/// let the conflict reach the operator.
///
/// Only the conflict is retried. A validation refusal or a storage failure is
/// returned on the first round: neither becomes true by trying again.
pub async fn commit_document<F>(transform: F) -> Result<String, CmdError>
where
    F: Fn(&Value) -> Result<Value, CmdError>,
{
    for _ in 0..COMMIT_ROUNDS {
        let (document, expected_generation) = fetch_versioned_document().await?;
        let next = transform(&document)?;
        if next == document {
            return Ok(expected_generation);
        }
        match push_document_if(&next, &expected_generation).await {
            Ok(generation) => return Ok(generation),
            Err(error) if error.code == REGISTRY_CONFLICT_EXIT => continue,
            Err(error) => return Err(error),
        }
    }
    Err(RegistryConflict {
        location: targets::registry_location(),
        expected: format!("whatever {COMMIT_ROUNDS} consecutive reads returned"),
        actual: RegistryActual::Raced,
    }
    .error())
}

/// Validate a candidate against the document it would replace.
///
/// An `inference` fault that this write does not touch is returned rather than
/// raised: see [`crate::targets::validate_registry_for_write`]. Reading the
/// current document is best-effort, because a store that cannot be read is
/// reported by the write itself a moment later, and failing here would just
/// move the same error earlier with a less useful sentence.
pub(in crate::cli::registry) async fn validate_for_write(
    document: &Value,
) -> Result<Option<String>, CmdError> {
    let current = match RegistryStore::open().await {
        Ok(store) => store
            .read_versioned()
            .await
            .ok()
            .flatten()
            .and_then(|blob| serde_json::from_str::<Value>(&blob.content).ok()),
        Err(_) => None,
    };
    crate::targets::validate_registry_for_write(document, current.as_ref()).map_err(|exc| {
        CmdError::click(exc.to_string()).stating(crate::failure::FailureCode::Refused)
    })
}

/// Say out loud that a pre-existing fault was carried past, so a scoped write
/// never looks like a clean one.
pub(in crate::cli::registry) fn warn_scoped_validation(pre_existing: Option<String>) {
    if let Some(detail) = pre_existing {
        eprintln!(
            "[registry] proceeding: this write leaves `inference` byte-identical, but that \
             section is already invalid and every write touching it will be refused until it \
             is repaired: {detail}"
        );
    }
}

/// Validate an in-memory document and compare-and-swap it against the
/// generation the caller read it at; returns the new generation.
///
/// The only programmatic write path. Validation runs BEFORE any store call,
/// so a document that would not validate never reaches the registry, and a
/// lost condition comes back as [`REGISTRY_CONFLICT_EXIT`] rather than a
/// generic failure — that is what lets [`commit_document`] retry a pure
/// transform and every other caller report the race instead of forcing.
pub async fn push_document_if(
    document: &Value,
    expected_generation: &str,
) -> Result<String, CmdError> {
    warn_scoped_validation(validate_for_write(document).await?);
    let payload = format!("{}\n", serde_json::to_string_pretty(document)?);
    let store = RegistryStore::open().await?;
    let generation = match store.compare_and_swap(expected_generation, &payload).await {
        Ok(generation) => generation,
        // Every backend reports both a moved generation and a missing object
        // this way, and neither tells this function which one it got, so the
        // sentence names what is certain: the document is not the one the
        // caller read.
        Err(StorageError::StorageConflict(_)) => {
            return Err(RegistryConflict {
                location: store.location().to_string(),
                expected: expected_generation.to_string(),
                actual: RegistryActual::Raced,
            }
            .error());
        }
        Err(error) => {
            return Err(CmdError::click(format!(
                "registry compare-and-swap failed: {error}"
            )));
        }
    };
    let confirmed = store
        .read_versioned()
        .await?
        .ok_or_else(|| CmdError::click("registry compare-and-swap verification found no object"))?;
    if confirmed.version != generation || confirmed.content != payload {
        return Err(CmdError::click(
            "registry compare-and-swap verification returned different bytes",
        ));
    }
    Ok(generation)
}

/// The canonical document and the generation it was read at, which together
/// are the only safe input to [`push_document_if`]: a generation from a
/// second read belongs to a possibly different document.
pub async fn fetch_versioned_document() -> Result<(Value, String), CmdError> {
    let store = RegistryStore::open().await?;
    let blob = store
        .read_versioned()
        .await?
        .ok_or_else(|| CmdError::click(format!("no registry document at {}", store.location())))?;
    let document: Value = serde_json::from_str(&blob.content)?;
    if !document.is_object() {
        return Err(CmdError::click(format!(
            "registry at {} is not an object",
            store.location()
        )));
    }
    Ok((document, blob.version))
}

/// The canonical registry as its raw document, off the same object
/// [`push_document_if`] compare-and-swaps.
///
/// Read-modify-write callers work on the raw document rather than on
/// [`Registry`](crate::targets::Registry) because an edit here is a surgical key change, and the raw
/// value is the shortest path to one. [`Registry`](crate::targets::Registry) is no longer lossy —
/// unmodelled top-level keys round-trip through `Registry::extra` and
/// `Registry::to_document` writes them back — so either route preserves the
/// document; this one simply does not re-serialize the parts it never
/// touched.
pub async fn fetch_document() -> Result<Value, CmdError> {
    let store = RegistryStore::open().await?;
    let text = store
        .read_text()
        .await?
        .ok_or_else(|| CmdError::click(format!("no registry document at {}", store.location())))?;
    let document: Value = serde_json::from_str(&text)?;
    if !document.is_object() {
        return Err(CmdError::click(format!(
            "registry at {} is not an object",
            store.location()
        )));
    }
    Ok(document)
}
