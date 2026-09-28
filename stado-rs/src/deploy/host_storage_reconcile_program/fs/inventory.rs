//! Walks of a storage root: its qualified `ecosystem/` objects, their
//! `.metadata` sidecars, and the whole physical tree. A symbolic link or any
//! entry that is not a regular file or directory stops the transaction; a
//! directory that cannot be listed is reported after the walk.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use serde_json::{json, Value};

use super::{join, metadata_name, metadata_path, real_directory, regular_identity};
use crate::deploy::host_storage_reconcile_program::{json::list, Context, Step};

/// The permission bits `stat.S_IMODE` keeps.
const MODE_BITS: u32 = 0o7777;

struct Labels {
    linked_directory: &'static str,
    irregular: &'static str,
    unreadable: &'static str,
}

struct Walk {
    files: Vec<(String, fs::Metadata)>,
    directories: Vec<String>,
}

/// Every regular file under `top` (depth first, not following links) and
/// every directory below it, both as absolute paths.
fn walk(top: &str, labels: &Labels) -> Step<Walk> {
    let mut found = Walk {
        files: Vec::new(),
        directories: Vec::new(),
    };
    let mut unreadable = Vec::new();
    let mut pending = vec![top.to_string()];
    while let Some(directory) = pending.pop() {
        let entries = match fs::read_dir(&directory) {
            Ok(entries) => entries,
            Err(error) => {
                unreadable.push(format!("{directory}: {error}"));
                continue;
            }
        };
        let mut children = Vec::new();
        for entry in entries {
            match entry {
                Ok(entry) => children.push(entry.path().to_string_lossy().into_owned()),
                Err(error) => unreadable.push(format!("{directory}: {error}")),
            }
        }
        children.sort();
        let mut files = Vec::new();
        for path in children {
            let info = fs::symlink_metadata(&path)
                .map_err(|error| format!("cannot inspect {path}: {error}"))?;
            if info.is_dir() {
                found.directories.push(path.clone());
                pending.push(path);
            } else if info.file_type().is_symlink()
                && fs::metadata(&path).is_ok_and(|to| to.is_dir())
            {
                return Err(format!("{}: {path}", labels.linked_directory));
            } else {
                files.push((path, info));
            }
        }
        for (path, info) in files {
            if !info.file_type().is_file() {
                return Err(format!("{}: {path}", labels.irregular));
            }
            found.files.push((path, info));
        }
    }
    if let Some(first) = unreadable.first() {
        return Err(format!("{}: {first}", labels.unreadable));
    }
    Ok(found)
}

fn relative(path: &str, root: &str) -> String {
    Path::new(path)
        .strip_prefix(root)
        .map(|inside| inside.to_string_lossy().into_owned())
        .unwrap_or_else(|_| path.to_string())
}

pub(in crate::deploy::host_storage_reconcile_program) fn object_paths(
    root: &str,
) -> Step<Vec<String>> {
    let ecosystem = join(root, "ecosystem");
    if !real_directory(&ecosystem) {
        return Err(format!("unsafe or absent ecosystem root: {ecosystem}"));
    }
    let labels = Labels {
        linked_directory: "symlinked object directory",
        irregular: "non-regular backup object",
        unreadable: "object enumeration failed",
    };
    let mut paths: Vec<String> = walk(&ecosystem, &labels)?
        .files
        .iter()
        .map(|(path, _)| relative(path, root))
        .collect();
    paths.sort();
    Ok(paths)
}

pub(in crate::deploy::host_storage_reconcile_program) fn metadata_paths(
    root: &str,
) -> Step<Vec<String>> {
    let metadata_root = join(root, ".metadata");
    let ecosystem = join(&metadata_root, "ecosystem");
    if fs::metadata(&ecosystem).is_err() {
        return Ok(Vec::new());
    }
    if !real_directory(&ecosystem) {
        return Err(format!("unsafe metadata ecosystem root: {ecosystem}"));
    }
    let labels = Labels {
        linked_directory: "symlinked metadata directory",
        irregular: "non-regular metadata object",
        unreadable: "metadata enumeration failed",
    };
    let mut paths: Vec<String> = walk(&ecosystem, &labels)?
        .files
        .iter()
        .map(|(path, _)| relative(path, &metadata_root))
        .collect();
    paths.sort();
    Ok(paths)
}

/// Every file of a root with its size, digest and mode, and every directory.
pub(in crate::deploy::host_storage_reconcile_program) fn physical_inventory(
    context: &Context,
    root: &str,
) -> Step<Value> {
    let labels = Labels {
        linked_directory: "symlinked physical-root directory",
        irregular: "non-regular physical-root entry",
        unreadable: "physical-root enumeration failed",
    };
    let walked = walk(root, &labels)?;
    let mut files = Vec::new();
    for (path, info) in &walked.files {
        files.push(json!({
            "path": relative(path, root),
            "body": {"bytes": info.len(), "sha256": super::digest(context, path)?},
            "mode": info.permissions().mode() & MODE_BITS,
        }));
    }
    files.sort_by(|left, right| left["path"].as_str().cmp(&right["path"].as_str()));
    let mut directories: Vec<String> = walked
        .directories
        .iter()
        .map(|path| relative(path, root))
        .collect();
    directories.sort();
    Ok(json!({"files": files, "directories": directories, "exclusions": []}))
}

fn path_and_body(inventory: &Value, label: &str) -> Step<Vec<Value>> {
    Ok(list(inventory, "files", label)?
        .iter()
        .map(|item| json!({"path": item.get("path"), "body": item.get("body")}))
        .collect())
}

pub(in crate::deploy::host_storage_reconcile_program) fn validate_physical_checkpoint(
    context: &Context,
    root: &str,
    snapshot: &Value,
    label: &str,
) -> Step<()> {
    let current = physical_inventory(context, root)?;
    if path_and_body(&current, label)? != path_and_body(snapshot, label)?
        || current.get("directories") != snapshot.get("directories")
    {
        return Err(format!("{label} physical root changed"));
    }
    Ok(())
}

fn inventory(context: &Context, root: &str, paths: &[String]) -> Step<Vec<Value>> {
    let mut objects = Vec::with_capacity(paths.len());
    for path in paths {
        let body_path = join(root, path);
        let body = regular_identity(context, &body_path)?;
        if body.is_null() {
            return Err(format!(
                "object vanished while it was inventoried: {body_path}"
            ));
        }
        objects.push(json!({
            "path": path,
            "body": body,
            "metadata": regular_identity(context, &metadata_path(root, path)?)?,
        }));
    }
    Ok(objects)
}

/// The qualified object paths of a root, their inventory, and its whole
/// physical tree.
pub(in crate::deploy::host_storage_reconcile_program) fn complete_physical_inventory(
    context: &Context,
    root: &str,
) -> Step<(Vec<Value>, Value)> {
    let paths = object_paths(root)?;
    let objects = inventory(context, root, &paths)?;
    Ok((objects, physical_inventory(context, root)?))
}

pub(in crate::deploy::host_storage_reconcile_program) fn item_path(item: &Value) -> Step<&str> {
    item.get("path")
        .and_then(Value::as_str)
        .ok_or_else(|| "an inventoried object has no path".to_string())
}

fn validate_inventory(context: &Context, root: &str, objects: &[Value], label: &str) -> Step<()> {
    for item in objects {
        let path = item_path(item)?;
        if Some(&regular_identity(context, &join(root, path))?) != item.get("body") {
            return Err(format!("{label} body changed: {path}"));
        }
        if Some(&regular_identity(context, &metadata_path(root, path)?)?) != item.get("metadata") {
            return Err(format!("{label} metadata changed: {path}"));
        }
    }
    Ok(())
}

pub(in crate::deploy::host_storage_reconcile_program) fn validate_complete_inventory(
    context: &Context,
    root: &str,
    objects: &[Value],
    label: &str,
) -> Step<()> {
    let expected = objects
        .iter()
        .map(|item| item_path(item).map(str::to_string))
        .collect::<Step<Vec<String>>>()?;
    if object_paths(root)? != expected {
        return Err(format!("{label} namespace changed"));
    }
    validate_inventory(context, root, objects, label)?;
    let mut expected_metadata = Vec::new();
    for item in objects {
        if item
            .get("metadata")
            .is_some_and(|metadata| !metadata.is_null())
        {
            let path = item_path(item)?;
            metadata_path(root, path)?;
            expected_metadata.push(metadata_name(path));
        }
    }
    expected_metadata.sort();
    if metadata_paths(root)? != expected_metadata {
        return Err(format!("{label} metadata namespace changed"));
    }
    Ok(())
}
