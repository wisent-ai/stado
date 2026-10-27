//! Every refusal a whole-document replace earns on its own contents, and the
//! document comparisons those refusals are made of.

use serde_json::{Map, Value};

use crate::cli::CmdError;

/// A registry document as an object, or the refusal that says why the text
/// is not one: a write of text that is not a JSON object would publish a
/// registry no reader can load, so the guard refuses it before the store
/// sees it, naming which document (the stored one or the upload) and why.
fn document(role: &str, text: &str) -> Result<Map<String, Value>, CmdError> {
    match serde_json::from_str::<Value>(text) {
        Ok(Value::Object(object)) => Ok(object),
        Ok(other) => Err(CmdError::refused(format!(
            "registry upload refused: {role} is JSON but not an object (it is {other}); a \
             registry is one object with targets and a service directory"
        ))),
        Err(error) => Err(CmdError::refused(format!(
            "registry upload refused: {role} is not JSON: {error}"
        ))),
    }
}

/// Top-level keys the outgoing document would delete from the object that is
/// already there.
///
/// A registry write is a whole-document replace, so a caller holding a stale
/// or differently-modelled copy silently deletes every key its own model does
/// not know about: the canonical document loses whole sections between one
/// read and the next, and gains a block no checkout in the tree models —
/// divergent builds writing the same object, each
/// erasing what it could not name. `targets::Registry` now keeps unmodelled
/// top-level keys in `extra`, and `fetch_document` hands read-modify-write
/// callers the raw document; this is the backstop for a payload that came
/// from neither.
///
/// Only removals are reported. Additions are how the document grows, and a
/// changed value is an edit rather than a loss.
fn removed_top_level_keys(before: &Map<String, Value>, after: &Map<String, Value>) -> Vec<String> {
    before
        .keys()
        .filter(|key| !after.contains_key(*key))
        .cloned()
        .collect()
}

/// The service directory's own publication counter, when the document carries
/// one.
///
/// `ServiceDirectory::generation` is what a consumer compares against the copy
/// it cached, and `ServiceDirectoryError::Stale` is the answer it gets when its
/// copy is older. That check only means something if the number never goes
/// backwards at the authority.
fn service_directory_generation(document: &Map<String, Value>) -> Option<u64> {
    document
        .get("service_directory")?
        .get("generation")?
        .as_u64()
}

/// The service directory itself, with its counter removed, so two documents
/// can be compared for whether the DECLARATIONS differ independently of the
/// number that is supposed to announce that they do.
fn service_directory_body(document: &Map<String, Value>) -> Option<Value> {
    let mut directory = document.get("service_directory")?.clone();
    directory.as_object_mut()?.remove("generation");
    Some(directory)
}

/// Whether a registry document declares at least one target, or `None` when
/// it carries no `targets` array.
fn declares_targets(document: &Map<String, Value>) -> Option<bool> {
    Some(!document.get("targets")?.as_array()?.is_empty())
}

/// Every refusal a whole-document replace earns on its own contents, run
/// before anything is written and independently of whose generation the swap
/// will spend.
///
/// `--if-generation` answers "is this an edit to the document that is there";
/// these answer "is this a document worth having at all", and a caller with a
/// perfectly current token still gets refused by them.
pub(super) fn refuse_unsafe_replace(
    current: Option<&crate::queue::VersionedText>,
    payload: &str,
    allow_removals: bool,
    allow_empty_fleet: bool,
) -> Result<(), CmdError> {
    let after = document("the uploaded document", payload)?;
    let Some(blob) = current else {
        // Nothing is there yet, so nothing can be lost or reverted.
        return Ok(());
    };
    let before = document(
        &format!("the stored registry at generation {}", blob.version),
        &blob.content,
    )?;
    if !allow_removals {
        let removed = removed_top_level_keys(&before, &after);
        if !removed.is_empty() {
            return Err(CmdError::refused(format!(
                "registry upload refused: it would delete the top-level key(s) {} \
                 that generation {} carries. A registry write replaces the whole \
                 document, so this is what a stale copy or a build that does not \
                 model those keys does to them. Re-pull, re-apply the edit, and \
                 push again; pass --force only if the deletion is the intent.",
                removed.join(", "),
                blob.version
            )));
        }
        // Same accident as the deleted-key guard above, one level in: the
        // whole document is replaced, so a writer holding an older copy
        // publishes its older directory over a newer one and every
        // consumer's staleness check silently starts agreeing with it.
        // A directory going from a later generation back to an earlier
        // one reverts every endpoint corrected in between.
        if let (Some(before_generation), Some(after_generation)) = (
            service_directory_generation(&before),
            service_directory_generation(&after),
        ) {
            if after_generation < before_generation {
                return Err(CmdError::refused(format!(
                    "registry upload refused: its service directory is generation \
                     {after_generation} and the registry already carries {before_generation}. \
                     The counter consumers use to detect a stale directory would go backwards, \
                     so every cached copy older than {before_generation} would start looking \
                     current. Re-pull, re-apply the edit, and push again; pass \
                     --force only if publishing the older directory is the intent."
                )));
            }
            // The same lost update one notch subtler: a corrected
            // endpoint is published, and a writer holding a copy from
            // before it pushes its own directory back at the SAME
            // generation. The decrease guard above never fires, every
            // consumer's staleness check agrees with the reverted copy,
            // and the correction is gone with nothing recording that it
            // had been.
            //
            // `push --if-generation` now refuses that write outright, and
            // read-modify-write callers have always used `push_document_if`
            // and the store's real CAS. This guard is what still catches
            // the caller who brought no token at all: a file carries no
            // provenance on its own, so the directory states the rule for
            // itself -- changing a declaration means advancing the counter
            // that announces the change.
            //
            // Only a CHANGED directory is refused. A writer that leaves
            // it byte-identical -- `release promote` rewriting
            // `release_control`, every fleet and enrollment edit -- is
            // untouched.
            if after_generation == before_generation
                && service_directory_body(&before) != service_directory_body(&after)
            {
                return Err(CmdError::refused(format!(
                    "registry upload refused: it changes the service directory but leaves \
                     its generation at {after_generation}, the number the registry already \
                     carries. Consumers compare that counter against the copy they cached, so a \
                     changed directory published under an unchanged one is invisible to \
                     every one of them -- and if your copy predates a correction, this \
                     write reverts it silently. Re-pull, re-apply the edit, advance \
                     service_directory.generation, and push again; pass --force only if \
                     publishing a changed directory under the same generation is the \
                     intent."
                )));
            }
        }
    }
    // Replacing a populated fleet with an empty one needs its own authorization.
    // `--force` permits key or generation changes, not removal of every target.
    if !allow_empty_fleet {
        if let (Some(true), Some(false)) = (declares_targets(&before), declares_targets(&after)) {
            return Err(CmdError::refused(format!(
                "registry upload refused: generation {} declares targets and this document \
                 declares none. --force does not authorize removing every target; pass \
                 --allow-empty-fleet if that deletion is intended.",
                blob.version
            )));
        }
    }
    Ok(())
}
