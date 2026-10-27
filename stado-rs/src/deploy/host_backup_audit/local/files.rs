//! What the host half of the backup audit reads of one file, and the walk
//! over a tree. Nothing here follows a symbolic link: a link is not the
//! object, and the one thing the pass may never do is unlink something whose
//! counterpart it did not actually read.

use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

/// What reading one path found.
#[derive(Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum FileState {
    /// A regular file, read (and hashed when asked).
    Present,
    /// Nothing at the path.
    Absent,
    /// Something at the path that is not a regular file.
    NotRegular,
    /// The metadata or the content could not be read.
    Unreadable,
}

impl FileState {
    /// No file could be compared: none is there, or it could not be read.
    pub(super) fn missing_or_unreadable(self) -> bool {
        self == Self::Absent || self == Self::Unreadable
    }
}

/// The word the marker lines carry, from the serde mapping.
impl std::fmt::Display for FileState {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        crate::primitives::word::write(self, formatter)
    }
}

/// A file's state, its byte count when its metadata was read, its content
/// digest when it was hashed, and why it could not be read when it could not.
pub(super) struct Identity {
    pub(super) state: FileState,
    pub(super) bytes: Option<u64>,
    pub(super) digest: Option<String>,
    pub(super) cause: Option<String>,
}

impl Identity {
    fn of(state: FileState, bytes: Option<u64>) -> Self {
        Identity {
            state,
            bytes,
            digest: None,
            cause: None,
        }
    }

    fn unreadable(bytes: Option<u64>, cause: String) -> Self {
        Identity {
            state: FileState::Unreadable,
            bytes,
            digest: None,
            cause: Some(cause),
        }
    }

    /// The size field of a marker row: the byte count, or nothing where no
    /// metadata was read, which the operator side reads as no size.
    pub(super) fn size(&self) -> String {
        match self.bytes {
            Some(bytes) => bytes.to_string(),
            None => String::new(),
        }
    }

    /// The digest field of a marker row: the content digest, or nothing
    /// where the content was not hashed.
    pub(super) fn digest_field(&self) -> &str {
        match &self.digest {
            Some(digest) => digest,
            None => "",
        }
    }

    /// The state as a sentence names it, with why it could not be read.
    pub(super) fn described(&self) -> String {
        match &self.cause {
            Some(cause) => format!("{} ({cause})", self.state),
            None => self.state.to_string(),
        }
    }
}

/// The SHA-256 of `path`'s content, through the crate's one digest reader.
pub(super) fn sha256(path: &Path) -> std::io::Result<String> {
    Ok(crate::primitives::digest::sha256_file(path)?.sha256)
}

/// The state and size of `path` without reading it; `hash` also reads it.
pub(super) fn identity(path: &Path, hash: bool) -> Identity {
    let entry = match fs::symlink_metadata(path) {
        Ok(entry) => entry,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Identity::of(FileState::Absent, None)
        }
        Err(error) => return Identity::unreadable(None, error.to_string()),
    };
    let bytes = Some(entry.size());
    if !entry.file_type().is_file() {
        return Identity::of(FileState::NotRegular, bytes);
    }
    if !hash {
        return Identity::of(FileState::Present, bytes);
    }
    match sha256(path) {
        Ok(digest) => Identity {
            state: FileState::Present,
            bytes,
            digest: Some(digest),
            cause: None,
        },
        Err(error) => Identity::unreadable(bytes, error.to_string()),
    }
}

/// The metadata document of an object path in a store.
pub(super) fn metadata_path(root: &Path, relative: &str) -> PathBuf {
    let name = if relative.ends_with(".json") {
        relative.to_string()
    } else {
        format!("{relative}.json")
    };
    root.join(".metadata").join(name)
}

/// Every non-directory entry under `root`, directories in name order, files
/// in name order within each directory. A symbolic link to a directory is not
/// descended into and not listed; it is reported through `skipped`. An
/// unreadable directory, or an entry of one that cannot be read, is reported
/// through `failed`, so the caller knows the walk did not see everything.
pub(super) fn walk(
    root: &Path,
    visit: &mut dyn FnMut(&Path),
    skipped: &mut dyn FnMut(&Path),
    failed: &mut dyn FnMut(String),
) {
    let entries = match fs::read_dir(root) {
        Ok(entries) => entries,
        Err(error) => {
            failed(format!("{}: {error}", root.display()));
            return;
        }
    };
    let mut directories = Vec::new();
    let mut files = Vec::new();
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                failed(format!(
                    "{}: an entry could not be read: {error}",
                    root.display()
                ));
                continue;
            }
        };
        let path = entry.path();
        match entry.file_type() {
            Ok(kind) if kind.is_dir() => directories.push(path),
            Ok(kind)
                if kind.is_symlink() && fs::metadata(&path).is_ok_and(|target| target.is_dir()) =>
            {
                skipped(&path)
            }
            _ => files.push(path),
        }
    }
    files.sort();
    directories.sort();
    for file in &files {
        visit(file);
    }
    for directory in &directories {
        walk(directory, visit, skipped, failed);
    }
}

/// A path the walk of `root` reached, relative to `root`, as text; one that
/// is not under `root` is the error naming both.
pub(super) fn relative(path: &Path, root: &Path) -> Result<String, String> {
    path.strip_prefix(root)
        .map(|inside| inside.to_string_lossy().into_owned())
        .map_err(|_| format!("{} is not under {}", path.display(), root.display()))
}
