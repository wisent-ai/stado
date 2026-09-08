//! Path resolution, listing and the shared I/O error shape for the
//! filesystem backend.

use std::fs;
use std::path::{Component, Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::queue::StorageError;

use super::LocalBackend;

impl LocalBackend {
    /// Resolve a blob path against the deployment root, rejecting escapes
    /// (Python `ValueError("storage path escapes deployment root")`).
    pub(super) fn path(&self, path: &str) -> Result<PathBuf, StorageError> {
        let target = normalize(&self.root.join(path));
        if !target.starts_with(&self.root) {
            return Err(StorageError::PathEscape(path.to_string()));
        }
        Ok(target)
    }

    pub(super) fn metadata_path(&self, path: &str) -> PathBuf {
        if Path::new(path)
            .extension()
            .is_some_and(|extension| extension == "json")
        {
            self.metadata.join(path)
        } else {
            self.metadata.join(format!("{path}.json"))
        }
    }

    /// Whether a filesystem path lives under `.locks/` or `.metadata/`
    /// (excluded from listings).
    fn is_internal(&self, path: &Path) -> bool {
        path.starts_with(&self.locks) || path.starts_with(&self.metadata)
    }

    /// SHA-256 hex version token for CAS.
    pub(super) fn version(data: &[u8]) -> String {
        hex::encode(Sha256::digest(data))
    }

    /// Root-relative `/`-joined names of every non-internal blob under
    /// `prefix`, unordered.
    ///
    /// Walk only the subtree the prefix names. Walking the whole root and
    /// filtering afterwards made every prefix listing cost the size of the
    /// store: `stado doctor` spent fifty seconds stat-ing 27k queue blobs
    /// to look at one diagnostics directory. Any relative path that starts
    /// with the prefix lives under its directory part, so the result set is
    /// unchanged.
    pub(super) fn relative_names(&self, prefix: &str) -> Result<Vec<String>, StorageError> {
        let (directory, _) = prefix.rsplit_once('/').unwrap_or(("", prefix));
        let scan_root = if directory.is_empty() {
            self.root.clone()
        } else {
            self.root.join(directory)
        };
        let mut files = Vec::new();
        if scan_root.is_dir() {
            walk(&scan_root, &mut files)?;
        }
        Ok(files
            .into_iter()
            .filter(|item| !self.is_internal(item))
            .filter_map(|item| {
                item.strip_prefix(&self.root).ok().map(|rel| {
                    rel.components()
                        .map(|c| c.as_os_str().to_string_lossy().into_owned())
                        .collect::<Vec<_>>()
                        .join("/")
                })
            })
            .filter(|rel| rel.starts_with(prefix))
            .collect())
    }
}

/// Preserve the operation and exact physical target for local I/O failures.
///
/// A bare `Permission denied (os error 13)` cannot distinguish an object body,
/// its metadata sidecar, or its lock. The path here is the durable target the
/// caller intended, even when the kernel refused creation of its temporary
/// sibling first.
pub(super) fn local_io(action: &str, target: &Path, error: std::io::Error) -> StorageError {
    StorageError::Other(format!(
        "local storage {action} {}: {error}",
        target.display()
    ))
}

/// Lexical path normalization: resolve `.` and `..` without touching the
/// filesystem (Python `Path.resolve(strict=False)` on POSIX, minus symlink
/// resolution).
pub(super) fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// Recursively collect files under `dir`.
fn walk(dir: &Path, out: &mut Vec<PathBuf>) -> std::io::Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            walk(&path, out)?;
        } else if path.is_file() {
            out.push(path);
        }
    }
    Ok(())
}

/// Python `Path.unlink(missing_ok=True)`.
pub(super) fn remove_missing_ok(path: &Path) -> Result<(), StorageError> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(err.into()),
    }
}
