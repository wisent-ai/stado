//! What the rollback phase restores one file and one directory set with: a
//! checkpointed file's bytes and mode put back (or the file removed when A
//! did not have it), and the directory list a physical inventory names.

use std::collections::BTreeSet;
use std::fs;
use std::os::unix::fs::PermissionsExt;

use serde_json::Value;

use super::{Context, Step};
use crate::deploy::host_storage_reconcile_program::fs::{clone_file, fsync_dir, parent_of};
use crate::deploy::host_storage_reconcile_program::json::list;

/// Put one file of live A back to its checkpointed bytes and mode, or
/// remove it when A did not have it.
pub(super) fn restore_file(
    context: &Context,
    destination: &str,
    snapshot_source: &str,
    before: &Value,
    original: Option<&Value>,
    omitted: &str,
) -> Step<()> {
    if before.is_null() {
        fs::remove_file(destination)
            .map_err(|error| format!("cannot remove {destination}: {error}"))?;
    } else {
        clone_file(context, snapshot_source, destination)?;
        // The checkpoint records the mode as a number inside the mode bits;
        // one absent is refused by the caller's sentence, one wider by name.
        let mode = match original
            .and_then(|file| file.get("mode"))
            .and_then(Value::as_u64)
        {
            None => return Err(omitted.to_string()),
            Some(mode) => u32::try_from(mode).map_err(|_wider| {
                format!("{destination}: the checkpoint records mode {mode}, wider than mode bits")
            })?,
        };
        fs::set_permissions(destination, fs::Permissions::from_mode(mode))
            .map_err(|error| format!("cannot restore the mode of {destination}: {error}"))?;
    }
    fsync_dir(&parent_of(destination)?)
}

/// The directories a physical inventory names. An inventory without the list,
/// or with an entry that is not a path, is refused: read as no directories,
/// a checkpoint missing its list made every directory now on disk look
/// created by the transaction, and the rollback removed each empty one.
pub(super) fn directories(inventory: &Value, which: &str) -> Step<BTreeSet<String>> {
    list(inventory, "directories", which)?
        .iter()
        .map(|entry| {
            entry
                .as_str()
                .map(str::to_string)
                .ok_or_else(|| format!("{which} names a directory that is not a path: {entry}"))
        })
        .collect()
}
