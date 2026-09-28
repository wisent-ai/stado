//! What the host half of the backup audit reads of one file, and the walk
//! over a tree. Nothing here follows a symbolic link: a link is not the
//! object, and the one thing the pass may never do is unlink something whose
//! counterpart it did not actually read.

use std::fs;
use std::io::Read;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

/// Bytes read per step while hashing one file.
const HASH_CHUNK: usize = 1 << 20;

/// A file's state, size and — when it was hashed — SHA-256, as the marker
/// lines spell them.
pub(super) struct Identity {
    pub(super) state: &'static str,
    pub(super) size: String,
    pub(super) digest: String,
}

impl Identity {
    fn of(state: &'static str, size: String) -> Self {
        Identity {
            state,
            size,
            digest: String::new(),
        }
    }
}

pub(super) fn sha256(path: &Path) -> std::io::Result<String> {
    let mut file = fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; HASH_CHUNK];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            return Ok(hex::encode(hasher.finalize()));
        }
        hasher.update(&buffer[..read]);
    }
}

/// The state and size of `path` without reading it; `hash` also reads it.
pub(super) fn identity(path: &Path, hash: bool) -> Identity {
    let entry = match fs::symlink_metadata(path) {
        Ok(entry) => entry,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Identity::of("absent", String::new())
        }
        Err(_) => return Identity::of("unreadable", String::new()),
    };
    let size = entry.size().to_string();
    if !entry.file_type().is_file() {
        return Identity::of("not_regular", size);
    }
    if !hash {
        return Identity::of("present", size);
    }
    match sha256(path) {
        Ok(digest) => Identity {
            state: "present",
            size,
            digest,
        },
        Err(_) => Identity::of("unreadable", size),
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
/// unreadable directory is reported through `failed`.
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
    for entry in entries.flatten() {
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

/// A path relative to `root`, as text.
pub(super) fn relative(path: &Path, root: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .into_owned()
}
