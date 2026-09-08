//! The reclamation: what is done to the tree of a job that is terminal.
//!
//! The two helpers below are the deletion itself — dir-fd-relative,
//! non-following, same-device, depth-bounded, and re-proving every directory
//! it descends into is still the entry it stat'd. `pass` is the cleaner entry
//! point that decides which trees reach here and spends the pass budget;
//! `legacy` is the compatibility root that pass ends with.

mod legacy;
pub(super) mod pass;

use std::collections::BTreeSet;
use std::ffi::{OsStr, OsString};
use std::os::fd::{AsRawFd, RawFd};

use nix::libc::dev_t;
use nix::sys::stat::FileStat;

use crate::providers::local::disk_cleanup::{ifmt, safefs, JanitorError, IFDIR};

const MAX_WORKDIR_DEPTH: usize = 256;

fn same_object(first: &FileStat, second: &FileStat) -> bool {
    first.st_dev == second.st_dev
        && first.st_ino == second.st_ino
        && first.st_mode == second.st_mode
        && first.st_uid == second.st_uid
}

fn entry_names(dir_fd: RawFd) -> Result<BTreeSet<OsString>, JanitorError> {
    let mut names = BTreeSet::new();
    for name in safefs::DirEntries::open(dir_fd)? {
        let name = name?;
        if name != "." && name != ".." {
            names.insert(name);
        }
    }
    Ok(names)
}

fn remove_contents_at(dir_fd: RawFd, root_dev: dev_t, depth: usize) -> Result<(), JanitorError> {
    for name in entry_names(dir_fd)? {
        let info = safefs::fstatat_nofollow(dir_fd, &name)?;
        if ifmt(info.st_mode as u32) != IFDIR {
            safefs::unlink_at(dir_fd, &name)?;
            continue;
        }
        if info.st_dev != root_dev {
            return Err(JanitorError::os("queue workdir spans a device boundary"));
        }
        if depth + 1 > MAX_WORKDIR_DEPTH {
            return Err(JanitorError::os("queue workdir nested too deeply"));
        }
        let child = safefs::open_dir_at(dir_fd, &name)?;
        if !same_object(&safefs::fstat(child.as_raw_fd())?, &info) {
            return Err(JanitorError::os(
                "queue workdir entry replaced while deleting",
            ));
        }
        remove_contents_at(child.as_raw_fd(), root_dev, depth + 1)?;
        drop(child);
        safefs::rmdir_at(dir_fd, &name)?;
    }
    Ok(())
}

fn remove_tree_at(
    root_fd: RawFd,
    name: &OsStr,
    work_fd: RawFd,
    root_dev: dev_t,
) -> Result<(), JanitorError> {
    remove_contents_at(work_fd, root_dev, 0)?;
    safefs::rmdir_at(root_fd, name)?;
    Ok(())
}
