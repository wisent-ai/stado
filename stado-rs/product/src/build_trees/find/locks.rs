//! Whether a build is writing a tree right now, read from the locks the
//! build tools hold while they write.

use std::{
    fs::{self, OpenOptions},
    io,
    os::unix::fs::OpenOptionsExt,
    path::{Path, PathBuf},
};

/// The lock files a build holds while it writes a tree: Stado's run marker
/// ([`crate::common::runs`]), SwiftPM's workspace lock and Cargo's build
/// directory lock.
const LOCKS: &[&str] = &[".in-use", ".lock", ".cargo-lock"];

/// The first lock a build holds in `tree`: at its top, in a directory below
/// it (a Stado run, a Cargo profile) or one level further (a Cargo profile
/// under a target triple). A directory or lock that cannot be read is
/// answered as the holder and named in `errors`: read as unlocked, a tree a
/// build was writing was removed from under it.
pub(super) fn holder(tree: &Path, errors: &mut Vec<String>) -> Option<PathBuf> {
    let mut directories = vec![tree.to_path_buf()];
    let children = match subdirectories(tree) {
        Ok(children) => children,
        Err(error) => {
            errors.push(format!("checking locks in {}: {error}", tree.display()));
            return Some(tree.to_path_buf());
        }
    };
    for child in children {
        match subdirectories(&child) {
            Ok(grandchildren) => directories.extend(grandchildren),
            Err(error) => {
                errors.push(format!("checking locks in {}: {error}", child.display()));
                return Some(child);
            }
        }
        directories.push(child);
    }
    for lock in directories
        .iter()
        .flat_map(|directory| LOCKS.iter().map(move |lock| directory.join(lock)))
    {
        match held(&lock) {
            Ok(false) => {}
            Ok(true) => return Some(lock),
            Err(error) => {
                errors.push(format!("checking lock {}: {error}", lock.display()));
                return Some(lock);
            }
        }
    }
    None
}

fn subdirectories(directory: &Path) -> io::Result<Vec<PathBuf>> {
    let mut found = Vec::new();
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        if entry.metadata()?.file_type().is_dir() {
            found.push(entry.path());
        }
    }
    Ok(found)
}

/// Whether a process holds the lock file at `path`: a shared lock this
/// process cannot take is a build's exclusive one. No lock file is not held;
/// one that cannot be opened is the error.
fn held(path: &Path) -> io::Result<bool> {
    match OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
    {
        Ok(file) => match file.try_lock_shared() {
            Ok(()) => Ok(false),
            Err(std::fs::TryLockError::WouldBlock) => Ok(true),
            Err(std::fs::TryLockError::Error(error)) => Err(error),
        },
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    }
}
