//! The effective lifecycle snapshot: the `ecosystem/probierz/` objects of
//! the A/B union, each conflict decided by the root that was primary before,
//! cloned from the sealed checkpoints into one sealed tree.

use std::collections::BTreeMap;
use std::fs;

use serde_json::Value;

use super::fs::{
    clone_file, fsync_dir, item_path, join, make_private_dirs, metadata_path, parent_of,
    regular_identity, seal_tree, set_tree_modes, validate_sealed_tree,
};
use super::{Context, Step};

const LIFECYCLE_ROOT: &str = "ecosystem/probierz/";

/// Every file of the snapshot, leaving out its `.locks` and `.metadata`
/// directories at any depth, is exactly the expected set.
fn validate(root: &str, expected: &[String]) -> Step<()> {
    let unreadable =
        |error: std::io::Error| format!("cannot read effective lifecycle snapshot: {error}");
    let mut actual = Vec::new();
    let mut pending = vec![root.to_string()];
    while let Some(directory) = pending.pop() {
        for entry in fs::read_dir(&directory).map_err(unreadable)? {
            let entry = entry.map_err(unreadable)?;
            let path = entry.path().to_string_lossy().into_owned();
            let info = fs::symlink_metadata(&path).map_err(unreadable)?;
            if info.is_dir() {
                let name = entry.file_name();
                if name != ".locks" && name != ".metadata" {
                    pending.push(path);
                }
                continue;
            }
            if info.file_type().is_symlink() && fs::metadata(&path).is_ok_and(|to| to.is_dir()) {
                continue;
            }
            if !info.file_type().is_file() {
                return Err(format!(
                    "effective lifecycle snapshot contains a non-regular file: {path}"
                ));
            }
            let inside = path.strip_prefix(root).unwrap_or(&path);
            actual.push(inside.trim_start_matches('/').to_string());
        }
    }
    actual.sort();
    if actual != expected {
        return Err(
            "effective lifecycle snapshot namespace differs from its qualified A/B union"
                .to_string(),
        );
    }
    Ok(())
}

/// Which object each snapshot path takes: `(source root, source path, item)`.
fn select(
    context: &Context,
    primary_objects: &[Value],
    backup_objects: &[Value],
    conflict_winner: &str,
) -> Step<BTreeMap<String, (String, String, Value)>> {
    let backup = (&context.backup_snapshot, backup_objects);
    let primary = (&context.primary_snapshot, primary_objects);
    // The later source overwrites the earlier, so the winner goes last.
    let sources = if conflict_winner == "primary" {
        [backup, primary]
    } else {
        [primary, backup]
    };
    let mut selected = BTreeMap::new();
    for (source_root, objects) in sources {
        for item in objects {
            let path = item_path(item)?;
            if let Some(inside) = path.strip_prefix(LIFECYCLE_ROOT) {
                selected.insert(
                    inside.to_string(),
                    (source_root.clone(), path.to_string(), item.clone()),
                );
            }
        }
    }
    Ok(selected)
}

pub(super) fn checkpoint_effective_lifecycle(
    context: &Context,
    primary_objects: &[Value],
    backup_objects: &[Value],
    conflict_winner: &str,
) -> Step<()> {
    let snapshot = &context.effective_lifecycle_snapshot;
    let selected = select(context, primary_objects, backup_objects, conflict_winner)?;
    let expected: Vec<String> = selected.keys().cloned().collect();
    let metadata_of = |item: &Value| item.get("metadata").cloned().unwrap_or(Value::Null);
    let body_of = |item: &Value| item.get("body").cloned().unwrap_or(Value::Null);
    if fs::metadata(snapshot).is_ok_and(|info| info.is_dir()) {
        validate_sealed_tree(snapshot)?;
        validate(snapshot, &expected)?;
        for (relative, (_, source_relative, item)) in &selected {
            if regular_identity(context, &join(snapshot, relative))? != body_of(item) {
                return Err(format!(
                    "effective lifecycle body differs from the immutable overlay: {source_relative}"
                ));
            }
            if regular_identity(context, &metadata_path(snapshot, relative)?)? != metadata_of(item)
            {
                return Err(format!(
                    "effective lifecycle metadata differs from the immutable overlay: {source_relative}"
                ));
            }
        }
        return Ok(());
    }
    let building = format!("{snapshot}.building");
    make_private_dirs(&join(&building, ".locks"))?;
    make_private_dirs(&join(&building, ".metadata"))?;
    for (relative, (source_root, source_relative, item)) in &selected {
        let destination = join(&building, relative);
        if regular_identity(context, &destination)? != body_of(item) {
            remove_if_present(&destination)?;
            clone_file(context, &join(source_root, source_relative), &destination)?;
        }
        let source_metadata = metadata_path(source_root, source_relative)?;
        let destination_metadata = metadata_path(&building, relative)?;
        if regular_identity(context, &destination_metadata)? != metadata_of(item) {
            remove_if_present(&destination_metadata)?;
            if !metadata_of(item).is_null() {
                clone_file(context, &source_metadata, &destination_metadata)?;
            }
        }
        if regular_identity(context, &destination)? != body_of(item) {
            return Err(format!(
                "effective lifecycle body did not verify: {source_relative}"
            ));
        }
        if regular_identity(context, &destination_metadata)? != metadata_of(item) {
            return Err(format!(
                "effective lifecycle metadata did not verify: {source_relative}"
            ));
        }
    }
    validate(&building, &expected)?;
    set_tree_modes(&building)?;
    fs::rename(&building, snapshot)
        .map_err(|error| format!("cannot move lifecycle snapshot to {snapshot}: {error}"))?;
    fsync_dir(&parent_of(snapshot))?;
    seal_tree(snapshot)?;
    validate_sealed_tree(snapshot)?;
    validate(snapshot, &expected)
}

fn remove_if_present(path: &str) -> Step<()> {
    if fs::symlink_metadata(path).is_ok() {
        fs::remove_file(path).map_err(|error| format!("cannot replace {path}: {error}"))?;
    }
    Ok(())
}
