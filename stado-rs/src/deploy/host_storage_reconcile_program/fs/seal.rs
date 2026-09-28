//! Immutable checkpoint trees: built beside their final name from verified
//! clones, made read-only, renamed into place and sealed with the owner
//! immutable flag, so nothing the transaction does later can change them.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use serde_json::Value;

use super::darwin;
use super::{
    clone_file, fsync_dir, join, make_private_dirs, parent_of, regular_identity,
    validate_physical_checkpoint,
};
use crate::deploy::host_storage_reconcile_program::json::list;
use crate::deploy::host_storage_reconcile_program::{Context, Step};

const SEALED_FILE: u32 = 0o400;
const SEALED_DIRECTORY: u32 = 0o500;
/// The permission bits `stat.S_IMODE` keeps.
const MODE_BITS: u32 = 0o7777;

/// One directory with its child directories and its files.
type Listing = (String, Vec<String>, Vec<String>);

/// Every directory under `root` with its child directories and files,
/// children before parents; links to directories are listed, not entered.
fn bottom_up(root: &str) -> Step<Vec<Listing>> {
    let mut order = Vec::new();
    let mut pending = vec![root.to_string()];
    while let Some(directory) = pending.pop() {
        let entries = fs::read_dir(&directory)
            .map_err(|error| format!("cannot list {directory}: {error}"))?;
        let (mut directories, mut files) = (Vec::new(), Vec::new());
        for entry in entries {
            let path = entry
                .map_err(|error| format!("cannot list {directory}: {error}"))?
                .path()
                .to_string_lossy()
                .into_owned();
            if fs::metadata(&path).is_ok_and(|info| info.is_dir()) {
                if fs::symlink_metadata(&path).is_ok_and(|info| info.is_dir()) {
                    pending.push(path.clone());
                }
                directories.push(path);
            } else {
                files.push(path);
            }
        }
        order.push((directory, directories, files));
    }
    order.reverse();
    Ok(order)
}

fn seal_one(path: &str, mode: u32) -> Step<()> {
    let info =
        fs::symlink_metadata(path).map_err(|error| format!("cannot inspect {path}: {error}"))?;
    let flags = darwin::flags(&info);
    if flags & darwin::USER_IMMUTABLE == 0 {
        fs::set_permissions(path, fs::Permissions::from_mode(mode))
            .and_then(|()| darwin::set_flags(path, flags | darwin::USER_IMMUTABLE))
            .map_err(|error| format!("cannot seal {path}: {error}"))?;
    }
    Ok(())
}

pub(in crate::deploy::host_storage_reconcile_program) fn seal_tree(root: &str) -> Step<()> {
    if !darwin::SUPPORTED {
        return Err("Darwin immutable-file flags are unavailable".to_string());
    }
    for (directory, directories, files) in bottom_up(root)? {
        for path in &files {
            seal_one(path, SEALED_FILE)?;
        }
        for path in &directories {
            seal_one(path, SEALED_DIRECTORY)?;
        }
        seal_one(&directory, SEALED_DIRECTORY)?;
    }
    Ok(())
}

fn sealed(path: &str, mode: u32) -> Step<bool> {
    let info =
        fs::symlink_metadata(path).map_err(|error| format!("cannot inspect {path}: {error}"))?;
    Ok(darwin::flags(&info) & darwin::USER_IMMUTABLE != 0
        && info.permissions().mode() & MODE_BITS == mode)
}

pub(in crate::deploy::host_storage_reconcile_program) fn validate_sealed_tree(
    root: &str,
) -> Step<()> {
    for (directory, directories, files) in bottom_up(root)? {
        for path in &files {
            if !sealed(path, SEALED_FILE)? {
                return Err(format!("checkpoint file is not immutable: {path}"));
            }
        }
        for path in &directories {
            if !sealed(path, SEALED_DIRECTORY)? {
                return Err(format!("checkpoint directory is not immutable: {path}"));
            }
        }
        if !sealed(&directory, SEALED_DIRECTORY)? {
            return Err(format!("checkpoint root is not immutable: {directory}"));
        }
    }
    Ok(())
}

/// Files read-only to the owner, directories listable and not writable.
pub(in crate::deploy::host_storage_reconcile_program) fn set_tree_modes(root: &str) -> Step<()> {
    for (directory, directories, files) in bottom_up(root)? {
        let modes = files
            .iter()
            .map(|path| (path, SEALED_FILE))
            .chain(directories.iter().map(|path| (path, SEALED_DIRECTORY)))
            .chain(std::iter::once((&directory, SEALED_DIRECTORY)));
        for (path, mode) in modes {
            fs::set_permissions(path, fs::Permissions::from_mode(mode))
                .map_err(|error| format!("cannot set the mode of {path}: {error}"))?;
        }
    }
    Ok(())
}

/// The immutable copy of `source`'s physical tree at `destination`, exactly
/// as `snapshot` inventoried it.
pub(in crate::deploy::host_storage_reconcile_program) fn checkpoint_tree(
    context: &Context,
    source: &str,
    destination: &str,
    snapshot: &Value,
) -> Step<()> {
    if fs::metadata(destination).is_ok_and(|info| info.is_dir()) {
        seal_tree(destination)?;
        validate_sealed_tree(destination)?;
        return validate_physical_checkpoint(
            context,
            destination,
            snapshot,
            "immutable checkpoint",
        );
    }
    let building = format!("{destination}.building");
    make_private_dirs(&building)?;
    for relative in list(snapshot, "directories", "physical checkpoint")? {
        let relative = relative.as_str().unwrap_or_default();
        let directory = join(&building, relative);
        if fs::symlink_metadata(&directory).is_ok() && !Path::new(&directory).is_dir() {
            return Err(format!(
                "checkpoint directory collides with non-directory: {relative}"
            ));
        }
        make_private_dirs(&directory)?;
    }
    for item in list(snapshot, "files", "physical checkpoint")? {
        let relative = item.get("path").and_then(Value::as_str).unwrap_or_default();
        let target = join(&building, relative);
        if Some(&regular_identity(context, &target)?) != item.get("body") {
            if fs::symlink_metadata(&target).is_ok() {
                fs::remove_file(&target)
                    .map_err(|error| format!("cannot replace {target}: {error}"))?;
            }
            clone_file(context, &join(source, relative), &target)?;
        }
        if Some(&regular_identity(context, &target)?) != item.get("body") {
            return Err(format!(
                "physical checkpoint file did not verify: {relative}"
            ));
        }
    }
    validate_physical_checkpoint(context, &building, snapshot, "building checkpoint")?;
    set_tree_modes(&building)?;
    fs::rename(&building, destination)
        .map_err(|error| format!("cannot move checkpoint to {destination}: {error}"))?;
    fsync_dir(&parent_of(destination))?;
    seal_tree(destination)?;
    validate_sealed_tree(destination)?;
    validate_physical_checkpoint(context, destination, snapshot, "sealed checkpoint")
}
