//! The store inventory: what the release namespace holds, and what still
//! names it.
//!
//! [`signed`] reads whether one version directory holds a complete signed
//! release; [`pins`] the versions this host's release agent, every target in
//! the registry and an operator's config files still name; [`runs`] the
//! publication evidence the pipeline run records leave behind. `tree_bytes`
//! below is how much disk one version directory occupies.

pub(super) mod pins;
pub(super) mod runs;
pub(super) mod signed;

use std::path::Path;

/// Bytes under a directory, counting plain files only. A directory, entry or
/// file that cannot be read is left out of the count and named on stderr, so
/// a reclaim that reports fewer bytes than it freed says why.
pub(in crate::providers::local::disk_cleanup::release_store) fn tree_bytes(root: &Path) -> i64 {
    let mut total = 0i64;
    let mut stack = vec![root.to_path_buf()];
    while let Some(directory) = stack.pop() {
        let entries = match std::fs::read_dir(&directory) {
            Ok(entries) => entries,
            Err(error) => {
                eprintln!(
                    "release store: {} is not counted: {error}",
                    directory.display()
                );
                continue;
            }
        };
        for entry in entries {
            let entry = match entry {
                Ok(entry) => entry,
                Err(error) => {
                    eprintln!(
                        "release store: an entry of {} is not counted: {error}",
                        directory.display()
                    );
                    continue;
                }
            };
            let path = entry.path();
            let counted = match entry.file_type() {
                Ok(kind) if kind.is_dir() => {
                    stack.push(path);
                    continue;
                }
                Ok(kind) if kind.is_file() => entry.metadata().map(|info| info.len() as i64),
                Ok(_) => continue,
                Err(error) => Err(error),
            };
            match counted {
                Ok(bytes) => total += bytes,
                Err(error) => {
                    eprintln!("release store: {} is not counted: {error}", path.display())
                }
            }
        }
    }
    total
}
