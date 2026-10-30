//! Whether a build is running in a tagged cache, read from the lock its
//! build tool holds, and the hold the cleaner keeps while it reclaims one.
//!
//! Cargo writes `CACHEDIR.TAG` into its build directory and holds an
//! exclusive advisory lock on a lock file of each profile directory for as
//! long as a build runs there, so the tag alone is no leave to delete: a
//! build directory reclaimed under a running build loses object files the
//! build is about to link. Which file it locks depends on the Cargo release
//! ([`LOCK_FILES`]): a build directory under Cargo's home, from a release
//! that lays its `build-dir` out apart from the artifacts, is held through
//! `.cargo-build-lock` and never touches `.cargo-lock`, so a guard that read
//! only `.cargo-lock` took a directory a running `cargo test` was compiling
//! into. Before a cache is removed, every one of those files at its root and
//! one directory below is locked the same way, without waiting. A lock
//! another process holds keeps the cache for a later pass; the locks taken
//! are held until the removal ends, so a build that starts meanwhile waits on
//! Cargo's own lock instead of building into a tree that is being deleted.

use std::ffi::{OsStr, OsString};
use std::os::fd::{AsRawFd, OwnedFd, RawFd};

use nix::fcntl::OFlag;
use nix::sys::stat::Mode;

use super::entry_names;
use crate::providers::local::space_reclaim::{ifmt, safefs, JanitorError, IFDIR};

/// Cargo's build locks: the files whose exclusive lock a running build
/// holds, in every layout a Cargo release writes. `.cargo-lock` and
/// `.cargo-artifact-lock` guard a target directory's profile; a
/// `build-dir` apart from the artifacts is guarded by `.cargo-build-lock`.
const LOCK_FILES: &[&str] = &[".cargo-lock", ".cargo-artifact-lock", ".cargo-build-lock"];

/// What locking a cache's build locks found.
pub(super) enum Holding {
    /// No other process holds one; the locks taken stay held while this
    /// value lives.
    Free(Vec<OwnedFd>),
    /// The directories whose build lock another process holds.
    Busy(Vec<OsString>),
}

/// Lock every build lock in `dir_fd`: the descriptors holding the ones
/// present, or `Err` naming nothing when another process holds one.
fn lock_in(dir_fd: RawFd) -> Result<Result<Vec<OwnedFd>, ()>, JanitorError> {
    let mut held = Vec::new();
    for name in LOCK_FILES {
        let file = match safefs::open_file_at(
            dir_fd,
            OsStr::new(name),
            OFlag::O_RDWR | OFlag::O_CLOEXEC,
            Mode::empty(),
        ) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error.into()),
        };
        // SAFETY: a descriptor this function owns; flock only takes an
        // advisory lock on it and returns an errno.
        let locked =
            unsafe { nix::libc::flock(file.as_raw_fd(), nix::libc::LOCK_EX | nix::libc::LOCK_NB) };
        if locked.is_negative() {
            let error = std::io::Error::last_os_error();
            if error.kind() == std::io::ErrorKind::WouldBlock {
                return Ok(Err(()));
            }
            return Err(error.into());
        }
        held.push(file);
    }
    Ok(Ok(held))
}

/// Lock every build lock of the cache at `dir_fd`: its own, and each
/// directory's directly inside it.
pub(super) fn take(dir_fd: RawFd) -> Result<Holding, JanitorError> {
    let mut held = Vec::new();
    let mut busy = Vec::new();
    match lock_in(dir_fd)? {
        Ok(files) => held.extend(files),
        Err(()) => busy.push(OsString::from(".")),
    }
    for name in entry_names(dir_fd)? {
        let info = safefs::fstatat_nofollow(dir_fd, &name)?;
        if ifmt(info.st_mode as u32) != IFDIR {
            continue;
        }
        let child = safefs::open_dir_at(dir_fd, &name)?;
        match lock_in(child.as_raw_fd())? {
            Ok(files) => held.extend(files),
            Err(()) => busy.push(name),
        }
    }
    Ok(match busy.is_empty() {
        true => Holding::Free(held),
        false => Holding::Busy(busy),
    })
}
