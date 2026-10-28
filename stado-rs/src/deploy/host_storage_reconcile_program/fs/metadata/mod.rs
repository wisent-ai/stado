//! Where an object's metadata sidecar lives under a root's `.metadata`.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use super::super::Step;
use super::{join, parent_of};

/// The object's sidecar name under `.metadata`, relative to it.
pub(in crate::deploy::host_storage_reconcile_program) fn metadata_name(relative: &str) -> String {
    if relative.ends_with(".json") {
        relative.to_string()
    } else {
        format!("{relative}.json")
    }
}

/// Where an object's metadata sidecar lives; refuses a symlinked directory
/// on the way to it.
pub(in crate::deploy::host_storage_reconcile_program) fn metadata_path(
    root: &str,
    relative: &str,
) -> Step<String> {
    let candidate = join(&join(root, ".metadata"), &metadata_name(relative));
    let directory = parent_of(&candidate)?;
    let inside = Path::new(&directory)
        .strip_prefix(root)
        .map_err(|error| format!("metadata path leaves its root: {candidate}: {error}"))?;
    let mut current = PathBuf::from(root);
    for component in inside.components() {
        current.push(component);
        match fs::symlink_metadata(&current) {
            Ok(info) if info.file_type().is_symlink() => {
                return Err(format!(
                    "symlinked metadata directory: {}",
                    current.display()
                ));
            }
            Ok(_directory) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(format!(
                    "cannot inspect metadata directory {}: {error}",
                    current.display()
                ))
            }
        }
    }
    Ok(candidate)
}
