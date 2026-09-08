//! `.locks` acquisition (Python `_hf_lock_state`).

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsStr;
use std::fs::File;
use std::io;
use std::os::fd::{AsRawFd, OwnedFd, RawFd};

use nix::fcntl::OFlag;
use nix::sys::stat::{FileStat, Mode};

use crate::providers::local::disk_cleanup::hf::{
    check_info, identity, os_error, stable_identity, Identity, LockScan, Parts, StableId,
};
use crate::providers::local::disk_cleanup::{
    ifmt, lock_contended, safefs, CleanupReport, JanitorError, ScanBudget, IFDIR, IFREG,
};

/// Walk the `.locks` tree, recording identities and (when `acquire`) taking
/// an exclusive flock on every regular lock file. Returns
/// (state, held lock files, present).
///
/// Python raises `BlockingIOError("cache lock held")` when any lock is
/// already held by a live download — the caller turns that into the
/// `cache_locked` skip and touches nothing.
pub(in crate::providers::local::disk_cleanup::hf) fn scan_lock_state(
    root_fd: RawFd,
    root_info: &FileStat,
    budget: &mut ScanBudget,
    report: &mut CleanupReport,
    acquire: bool,
    lock_name: &str,
    already_held: &BTreeSet<StableId>,
) -> Result<LockScan, JanitorError> {
    let mut state: BTreeMap<Parts, Identity> = BTreeMap::new();
    let mut held: Vec<File> = Vec::new();
    let locks_fd = match safefs::open_dir_at(root_fd, OsStr::new(lock_name)) {
        Ok(fd) => fd,
        Err(exc) if exc.kind() == io::ErrorKind::NotFound => return Ok((state, held, false)),
        Err(exc) => return Err(exc.into()),
    };
    let locks_info = safefs::fstat(locks_fd.as_raw_fd())?;
    check_info(&locks_info, root_info)?;
    state.insert(Vec::new(), identity(&locks_info));
    let mut stack: Vec<(Parts, OwnedFd)> = vec![(Vec::new(), locks_fd)];
    while let Some((prefix, directory_fd)) = stack.pop() {
        {
            for name in safefs::DirEntries::open(directory_fd.as_raw_fd())? {
                let name = name?;
                budget.tick(report)?;
                let info = safefs::fstatat_nofollow(directory_fd.as_raw_fd(), &name)?;
                check_info(&info, root_info)?;
                let mut relative = prefix.clone();
                relative.push(name.clone());
                let kind = ifmt(info.st_mode as u32);
                if kind == IFDIR {
                    let child = safefs::open_dir_at(directory_fd.as_raw_fd(), &name)?;
                    if identity(&safefs::fstat(child.as_raw_fd())?) != identity(&info) {
                        return Err(os_error("cache lock directory changed"));
                    }
                    state.insert(relative.clone(), identity(&info));
                    stack.push((relative, child));
                } else if kind == IFREG {
                    let descriptor = safefs::open_file_at(
                        directory_fd.as_raw_fd(),
                        &name,
                        OFlag::O_RDWR,
                        Mode::empty(),
                    )?;
                    let opened = safefs::fstat(descriptor.as_raw_fd())?;
                    if identity(&opened) != identity(&info) {
                        return Err(os_error("cache lock changed"));
                    }
                    state.insert(relative, identity(&opened));
                    if acquire && !already_held.contains(&stable_identity(&opened)) {
                        let file = File::from(descriptor);
                        match fs2::FileExt::try_lock_exclusive(&file) {
                            Ok(()) => held.push(file),
                            Err(exc) if lock_contended(&exc) => {
                                return Err(JanitorError::blocking("cache lock held"));
                            }
                            Err(exc) => return Err(exc.into()),
                        }
                    }
                } else {
                    return Err(os_error("unsafe cache lock"));
                }
            }
        }
    }
    Ok((state, held, true))
}
