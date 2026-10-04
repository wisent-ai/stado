//! Reclamation: the depth-first unlink of one tagged cache, through
//! directory descriptors no rename can redirect.

use std::ffi::OsStr;
use std::os::fd::{AsRawFd, RawFd};

use nix::libc::dev_t;

use super::{entry_names, same_object};
use crate::providers::local::disk_cleanup::{ifmt, safefs, JanitorError, IFDIR};

/// Delete the tree `name` names beneath `parent_fd`, contents first.
///
/// `dir_fd` is the already-validated descriptor for that same directory, so
/// the contents are removed through a handle no rename can redirect. Only
/// the final `rmdir` addresses the name again, and it removes a directory
/// only when empty — the worst a swap at that instant can do is fail.
pub(super) fn remove_tree(
    parent_fd: RawFd,
    name: &OsStr,
    dir_fd: RawFd,
    root_dev: dev_t,
) -> Result<(), JanitorError> {
    remove_contents(dir_fd, root_dev)?;
    safefs::rmdir_at(parent_fd, name)?;
    Ok(())
}

/// Unlink everything inside `dir_fd`, depth first. One descriptor is held per
/// level; a tree deeper than the process may open reports the system's own
/// refusal.
fn remove_contents(dir_fd: RawFd, root_dev: dev_t) -> Result<(), JanitorError> {
    for name in entry_names(dir_fd)? {
        let info = safefs::fstatat_nofollow(dir_fd, &name)?;
        // Symlinks, sockets, devices: unlinked as names, never traversed.
        if ifmt(info.st_mode as u32) != IFDIR {
            safefs::unlink_at(dir_fd, &name)?;
            continue;
        }
        if info.st_dev != root_dev {
            return Err(JanitorError::os("build cache spans a device boundary"));
        }
        let child = safefs::open_dir_at(dir_fd, &name)?;
        if !same_object(&safefs::fstat(child.as_raw_fd())?, &info) {
            return Err(JanitorError::os(
                "build cache entry replaced while deleting",
            ));
        }
        remove_contents(child.as_raw_fd(), root_dev)?;
        drop(child);
        safefs::rmdir_at(dir_fd, &name)?;
    }
    Ok(())
}
