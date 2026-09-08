//! The pre-deletion rechecks (Python `_hf_recheck_*`): nothing is unlinked
//! until the scan that selected it has been re-proved against the disk.

use std::collections::BTreeSet;
use std::ffi::OsString;
use std::os::fd::{AsRawFd, OwnedFd, RawFd};

use nix::fcntl::OFlag;
use nix::sys::stat::{FileStat, Mode};

use crate::providers::local::disk_cleanup::hf::inventory::snapshots::snapshot_state;
use crate::providers::local::disk_cleanup::hf::{
    check_info, identity, os_error, HfCandidate, Identity, RepoScan,
};
use crate::providers::local::disk_cleanup::{
    ifmt, safefs, CleanupReport, JanitorError, ScanBudget, IFDIR, IFREG,
};

/// Python `_hf_recheck_ref`.
pub(in crate::providers::local::disk_cleanup::hf) fn recheck_ref(
    root_fd: RawFd,
    path_parts: &[OsString],
    expected: &Identity,
    commit: &str,
) -> Result<(), JanitorError> {
    let parent_fd = safefs::open_path(root_fd, &path_parts[..path_parts.len() - 1])?;
    let result = (|parent_fd: &OwnedFd| {
        let info =
            safefs::fstatat_nofollow(parent_fd.as_raw_fd(), &path_parts[path_parts.len() - 1])?;
        if identity(&info) != *expected || ifmt(info.st_mode as u32) != IFREG {
            return Err(os_error("cache reference changed"));
        }
        let descriptor = safefs::open_file_at(
            parent_fd.as_raw_fd(),
            &path_parts[path_parts.len() - 1],
            OFlag::O_RDONLY,
            Mode::empty(),
        )?;
        let payload = {
            if identity(&safefs::fstat(descriptor.as_raw_fd())?) != *expected {
                return Err(os_error("cache reference changed"));
            }
            safefs::read_fd(descriptor.as_raw_fd(), 257)?
        };
        drop(descriptor);
        let matches_commit =
            payload.len() <= 256 && std::str::from_utf8(&payload).map(str::trim) == Ok(commit);
        if !matches_commit {
            return Err(os_error("cache reference retargeted"));
        }
        Ok(())
    })(&parent_fd);
    drop(parent_fd);
    result
}

/// Python `_hf_recheck_repository_snapshots`.
pub(in crate::providers::local::disk_cleanup::hf) fn recheck_repository_snapshots(
    root_fd: RawFd,
    root_info: &FileStat,
    scan: &RepoScan,
    budget: &mut ScanBudget,
    report: &mut CleanupReport,
) -> Result<(), JanitorError> {
    let live: Vec<&HfCandidate> = scan
        .candidates
        .iter()
        .filter(|item| !item.deleted)
        .collect();
    let repo = &scan
        .candidates
        .first()
        .map(|c| c.repo.clone())
        .unwrap_or_default();
    let mut snapshots_parts = repo.clone();
    snapshots_parts.push(OsString::from("snapshots"));
    let snapshots_fd = safefs::open_path(root_fd, &snapshots_parts)?;
    let mut names: BTreeSet<OsString> = BTreeSet::new();
    {
        for name in safefs::DirEntries::open(snapshots_fd.as_raw_fd())? {
            let name = name?;
            budget.tick(report)?;
            let info = safefs::fstatat_nofollow(snapshots_fd.as_raw_fd(), &name)?;
            check_info(&info, root_info)?;
            if ifmt(info.st_mode as u32) != IFDIR {
                return Err(os_error("snapshot set changed"));
            }
            names.insert(name);
        }
    }
    drop(snapshots_fd);
    if names != live.iter().map(|item| item.commit.clone()).collect() {
        return Err(os_error("snapshot set changed"));
    }
    for item in live {
        let (state, modified, expected, referenced) = snapshot_state(
            root_fd,
            root_info,
            &item.repo,
            &item.commit,
            &scan.blobs,
            budget,
            report,
        )?;
        if state != item.snapshot
            || modified != item.modified
            || expected != item.snapshot_expected
            || referenced != item.referenced_blobs
        {
            return Err(os_error("snapshot changed before deletion"));
        }
    }
    Ok(())
}
