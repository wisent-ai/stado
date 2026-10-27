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

    /// SHA-256 hex version token for CAS.
    pub(super) fn version(data: &[u8]) -> String {
        hex::encode(Sha256::digest(data))
    }

    /// Root-relative `/`-joined names of every non-internal blob under
    /// `prefix`, unordered.
    ///
    /// Walk only what the prefix can name: in the directory part, only the
    /// entries whose name starts with the prefix's last segment, and the
    /// whole subtree of each of those. Walking the prefix's whole directory
    /// made one object's prefix (an object read asks for its own metadata
    /// this way) cost every object beside it, and a namespace directory
    /// holds the whole queue. Any relative path that starts with the prefix
    /// starts, in that directory, with an entry whose name starts with the
    /// last segment, so the result set is unchanged.
    ///
    /// The walk runs on the runtime's blocking pool: a caller such as the
    /// object API serves every connection on one thread, and a directory
    /// walk done on it held every other request until it ended.
    pub(super) async fn relative_names(&self, prefix: &str) -> Result<Vec<String>, StorageError> {
        let root = self.root.clone();
        let internal = [self.locks.clone(), self.metadata.clone()];
        let wanted = prefix.to_owned();
        off_thread(prefix, move || names_under(&root, &internal, &wanted)).await
    }
}

/// One blocking filesystem step of the local store, about `path`, run on
/// the runtime's blocking pool so the thread that awaits it keeps serving: a
/// step that does not finish (it panicked, or the runtime is shutting down)
/// is refused naming the path.
pub(super) async fn off_thread<T: Send + 'static>(
    path: &str,
    work: impl FnOnce() -> Result<T, StorageError> + Send + 'static,
) -> Result<T, StorageError> {
    tokio::task::spawn_blocking(work).await.map_err(|error| {
        StorageError::Other(format!(
            "the local storage step on {path:?} did not finish: {error}"
        ))
    })?
}

/// The walk [`LocalBackend::relative_names`] describes, over `root`, leaving
/// out every path under one of `internal` (`.locks/`, `.metadata/`).
fn names_under(
    root: &Path,
    internal: &[PathBuf],
    prefix: &str,
) -> Result<Vec<String>, StorageError> {
    let (directory, leaf) = match prefix.rsplit_once('/') {
        Some(split) => split,
        None => ("", prefix),
    };
    let scan_root = if directory.is_empty() {
        root.to_path_buf()
    } else {
        root.join(directory)
    };
    let mut files = Vec::new();
    if scan_root.is_dir() {
        for entry in fs::read_dir(&scan_root)? {
            let entry = entry?;
            if !entry.file_name().to_string_lossy().starts_with(leaf) {
                continue;
            }
            let path = entry.path();
            if path.is_dir() {
                walk(&path, &mut files)?;
            } else if path.is_file() {
                files.push(path);
            }
        }
    }
    let mut names = Vec::new();
    for item in files {
        if internal.iter().any(|area| item.starts_with(area)) {
            continue;
        }
        let relative = item.strip_prefix(root).map_err(|error| {
            std::io::Error::other(format!(
                "the listed blob {} is not under the storage root {}: {error}",
                item.display(),
                root.display()
            ))
        })?;
        let name = relative
            .components()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join("/");
        if name.starts_with(prefix) {
            names.push(name);
        }
    }
    Ok(names)
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

/// When the blob at `target` was last modified; `None` when there is none.
pub(super) fn modified(
    target: &Path,
) -> Result<Option<chrono::DateTime<chrono::Utc>>, StorageError> {
    match target.metadata() {
        Ok(md) => Ok(Some(chrono::DateTime::<chrono::Utc>::from(md.modified()?))),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(err.into()),
    }
}

/// Bytes `first` through `last` of the file at `target`, as many of them as
/// it holds, read from that offset alone; `None` when there is no file.
pub(super) fn read_range(
    target: &Path,
    first: u64,
    last: u64,
) -> Result<Option<Vec<u8>>, StorageError> {
    use std::io::{Read, Seek, SeekFrom};
    let mut file = match fs::File::open(target) {
        Ok(file) => file,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(err.into()),
    };
    let span = last.checked_sub(first).ok_or_else(|| {
        StorageError::Other(format!(
            "bytes {first}-{last} of {} is no range: its last byte comes before its first",
            target.display()
        ))
    })?;
    // A range's first and last bytes are both in it, so it holds one byte
    // more than its span (a span to the last byte a file can hold reads to
    // its end): https://www.rfc-editor.org/rfc/rfc9110#section-14.1.2
    let wanted = span.saturating_add(1);
    file.seek(SeekFrom::Start(first))?;
    let mut bytes = Vec::new();
    file.take(wanted).read_to_end(&mut bytes)?;
    Ok(Some(bytes))
}

/// A blob's metadata sidecar; no metadata only when there is no sidecar. One
/// that cannot be read or does not parse fails the call with its path:
/// merging over it would rewrite it holding only the new keys, and listing
/// it would show a blob without the metadata it carries.
pub(super) fn read_sidecar(
    path: &Path,
) -> Result<std::collections::BTreeMap<String, String>, StorageError> {
    let raw = match fs::read_to_string(path) {
        Ok(raw) => raw,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(std::collections::BTreeMap::new())
        }
        Err(error) => {
            return Err(std::io::Error::new(
                error.kind(),
                format!("cannot read blob metadata {}: {error}", path.display()),
            )
            .into())
        }
    };
    serde_json::from_str(&raw).map_err(|error| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!(
                "blob metadata {} is not a JSON object of strings: {error}",
                path.display()
            ),
        )
        .into()
    })
}
