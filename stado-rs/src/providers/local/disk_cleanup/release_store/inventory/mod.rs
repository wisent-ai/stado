//! The store inventory: what the release namespace holds, and what still
//! names it.
//!
//! [`families`] reads which release families one version directory completes;
//! [`pins`] the versions this host's release agent, every target in the
//! registry and an operator's config files still name; [`runs`] the
//! publication evidence the pipeline run records leave behind. `tree_bytes`
//! below is how much disk one version directory occupies.

pub(super) mod families;
pub(super) mod pins;
pub(super) mod runs;

use std::path::Path;

/// Bytes under a directory, counting plain files only.
pub(in crate::providers::local::disk_cleanup::release_store) fn tree_bytes(root: &Path) -> i64 {
    let mut total = 0i64;
    let mut stack = vec![root.to_path_buf()];
    while let Some(directory) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&directory) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            match entry.file_type() {
                Ok(kind) if kind.is_dir() => stack.push(path),
                Ok(kind) if kind.is_file() => {
                    total += entry.metadata().map(|m| m.len() as i64).unwrap_or_default();
                }
                _ => {}
            }
        }
    }
    total
}
