//! Every refusal a whole-document replace earns on its own contents, and the
//! document comparisons those refusals are made of.

use serde_json::Value;

use crate::cli::CmdError;

/// Top-level keys the outgoing document would delete from the object that is
/// already there.
///
/// A registry write is a whole-document replace, so a caller holding a stale
/// or differently-modelled copy silently deletes every key its own model does
/// not know about. That is not hypothetical: on 2026-08-04 the canonical
/// document lost `channels`, `enrollment` and `fleets` between one read and
/// the next, and gained a `service_directory` block no checkout in the tree
/// modelled at the time — divergent builds writing the same object, each
/// erasing what it could not name. `targets::Registry` now keeps unmodelled
/// top-level keys in `extra`, and `fetch_document` hands read-modify-write
/// callers the raw document; this is the backstop for a payload that came
/// from neither.
///
/// Only removals are reported. Additions are how the document grows, and a
/// changed value is an edit rather than a loss.
fn removed_top_level_keys(current: &str, payload: &str) -> Vec<String> {
    let (Ok(Value::Object(before)), Ok(Value::Object(after))) = (
        serde_json::from_str::<Value>(current),
        serde_json::from_str::<Value>(payload),
    ) else {
        return Vec::new();
    };
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
fn service_directory_generation(text: &str) -> Option<u64> {
    serde_json::from_str::<Value>(text)
        .ok()?
        .get("service_directory")?
        .get("generation")?
        .as_u64()
}

/// The service directory itself, with its counter removed, so two documents
/// can be compared for whether the DECLARATIONS differ independently of the
/// number that is supposed to announce that they do.
fn service_directory_body(text: &str) -> Option<Value> {
    let mut directory = serde_json::from_str::<Value>(text)
        .ok()?
        .get("service_directory")?
        .clone();
    directory.as_object_mut()?.remove("generation");
    Some(directory)
}

/// The number of targets a registry document declares, or `None` when the
/// text is not a document with a `targets` array.
fn target_count(text: &str) -> Option<usize> {
    Some(
        serde_json::from_str::<Value>(text)
            .ok()?
            .get("targets")?
            .as_array()?
            .len(),
    )
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
    if !allow_removals {
        if let Some(blob) = current {
            let removed = removed_top_level_keys(&blob.content, payload);
            if !removed.is_empty() {
                return Err(CmdError::click(format!(
                    "registry upload refused: it would delete the top-level key(s) {} \
                     that generation {} carries. A registry write replaces the whole \
                     document, so this is what a stale copy or a build that does not \
                     model those keys does to them. Re-pull, re-apply the edit, and \
                     push again; pass --force only if the deletion is the intent.",
                    removed.join(", "),
                    blob.version
                )));
            }
        }
        if let Some(blob) = current {
            // Same accident as the deleted-key guard above, one level in: the
            // whole document is replaced, so a writer holding an older copy
            // publishes its older directory over a newer one and every
            // consumer's staleness check silently starts agreeing with it.
            // Observed on 2026-08-12, when the directory went from generation
            // 10 back to 5 and two corrected endpoints reverted with it.
            if let (Some(before), Some(after)) = (
                service_directory_generation(&blob.content),
                service_directory_generation(payload),
            ) {
                if after < before {
                    return Err(CmdError::click(format!(
                        "registry upload refused: its service directory is generation \
                         {after} and the registry already carries {before}. The counter \
                         consumers use to detect a stale directory would go backwards, \
                         so every cached copy older than {before} would start looking \
                         current. Re-pull, re-apply the edit, and push again; pass \
                         --force only if publishing the older directory is the intent."
                    )));
                }
                // The same lost update one notch subtler, and the one that
                // actually happened. On 2026-09-01 a corrected brama endpoint
                // was published, and a writer holding a copy from before it
                // pushed its own directory back at the SAME generation. The
                // decrease guard above never fired, every consumer's
                // staleness check agreed with the reverted copy, and the
                // correction was gone with nothing recording that it had
                // been.
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
                if after == before
                    && service_directory_body(&blob.content) != service_directory_body(payload)
                {
                    return Err(CmdError::click(format!(
                        "registry upload refused: it changes the service directory but leaves \
                         its generation at {after}, the number the registry already carries. \
                         Consumers compare that counter against the copy they cached, so a \
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
    }
    // The floor `--force` may not cross. Every other guard here answers "did
    // the caller mean to drop this?"; this one answers "is this a fleet at
    // all?", and no legitimate edit to a three-host registry leaves zero
    // targets. On 2026-09-01 a worker ran
    // `stado registry push --force < /tmp/registry_updated.json`: the command
    // takes a PATH, so stdin was never read, `source_path(None)` resolved to
    // the repository's bundled `data/fleet/registry.json` - 65 bytes,
    // `{"schema_version":2,"coordinators":[],"targets":[]}` - and `--force`
    // waved it past the deleted-key guard that had refused the first attempt.
    // The live document lost all three targets, all eighteen of the mini's
    // service declarations, and the `fleets`, `inference`,
    // `placement_profiles`, `release_control` and `service_directory` keys.
    // `stado service reap` then answered that the always-on Mac is not in the
    // canonical registry.
    if !allow_empty_fleet {
        if let Some(blob) = current {
            if let (Some(before), Some(after)) =
                (target_count(&blob.content), target_count(payload))
            {
                if before > 0 && after == 0 {
                    return Err(CmdError::click(format!(
                        "registry upload refused: generation {} declares {before} target(s) and \
                         this document declares none. A fleet does not shrink to zero by edit, \
                         so this is an empty or wrong file, not an intention - most often the \
                         bundled skeleton reached through a missing path argument. --force does \
                         NOT cross this floor: pass --allow-empty-fleet if erasing every target \
                         is genuinely what you mean.",
                        blob.version
                    )));
                }
            }
        }
    }
    Ok(())
}
