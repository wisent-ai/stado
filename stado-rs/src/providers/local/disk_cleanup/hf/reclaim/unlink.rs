//! The identity-checked removal of one candidate (Python
//! `_hf_unlink_checked` / `_hf_execute_candidate`).

use std::ffi::OsString;
use std::os::fd::{AsRawFd, OwnedFd, RawFd};

use crate::providers::local::disk_cleanup::hf::{identity, os_error, HfCandidate, Identity, Parts};
use crate::providers::local::disk_cleanup::{ifmt, safefs, JanitorError, IFDIR};

/// Python `_hf_unlink_checked`: re-stat the leaf immediately before
/// removing it and refuse when the identity drifted. Directories may only
/// drift in size/mtime/nlink (their entries were already removed).
fn unlink_checked(
    root_fd: RawFd,
    path_parts: &[OsString],
    expected: &Identity,
    directory: bool,
) -> Result<(), JanitorError> {
    let parent_fd = safefs::open_path(root_fd, &path_parts[..path_parts.len() - 1])?;
    let result = (|parent_fd: &OwnedFd| {
        let leaf = &path_parts[path_parts.len() - 1];
        let info = safefs::fstatat_nofollow(parent_fd.as_raw_fd(), leaf)?;
        let current = identity(&info);
        if current != *expected && !(directory && current.stable() == expected.stable()) {
            return Err(os_error("cache entry changed before deletion"));
        }
        if directory {
            if ifmt(info.st_mode as u32) != IFDIR {
                return Err(os_error("cache directory changed type"));
            }
            safefs::rmdir_at(parent_fd.as_raw_fd(), leaf)?;
        } else {
            if ifmt(info.st_mode as u32) == IFDIR {
                return Err(os_error("cache file changed type"));
            }
            safefs::unlink_at(parent_fd.as_raw_fd(), leaf)?;
        }
        Ok(())
    })(&parent_fd);
    drop(parent_fd);
    result
}

/// Python `_hf_execute_candidate`: refs first, then the snapshot tree
/// deepest-first, then the exclusive blobs.
pub(in crate::providers::local::disk_cleanup::hf) fn execute_candidate(
    root_fd: RawFd,
    candidate: &HfCandidate,
) -> Result<(), JanitorError> {
    for (path_parts, ident) in &candidate.refs {
        unlink_checked(root_fd, path_parts, ident, false)?;
    }
    let mut snapshot_root = candidate.repo.clone();
    snapshot_root.push(OsString::from("snapshots"));
    snapshot_root.push(candidate.commit.clone());
    let mut entries: Vec<(&Parts, &Identity)> = candidate.snapshot.iter().collect();
    entries.sort_by_key(|(parts, _)| std::cmp::Reverse(parts.len()));
    for (relative, ident) in entries {
        let mut path_parts = snapshot_root.clone();
        path_parts.extend(relative.iter().cloned());
        unlink_checked(root_fd, &path_parts, ident, ident.ifmt == IFDIR)?;
    }
    for (path_parts, ident) in &candidate.delete_blobs {
        unlink_checked(root_fd, path_parts, ident, false)?;
    }
    Ok(())
}
