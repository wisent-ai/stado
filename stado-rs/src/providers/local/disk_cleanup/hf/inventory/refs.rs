//! The commit pointers under `refs/` and the reserved `.no_exist/` metadata.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::io;
use std::os::fd::{AsRawFd, OwnedFd, RawFd};
use std::os::unix::ffi::OsStrExt;

use nix::fcntl::OFlag;
use nix::sys::stat::{FileStat, Mode};

use crate::providers::local::disk_cleanup::hf::{check_info, identity, os_error, Identity, Parts};
use crate::providers::local::disk_cleanup::{
    ifmt, safefs, CleanupReport, JanitorError, ScanBudget, IFDIR, IFREG,
};

/// Python `_hf_scan_refs`: commit -> [(ref path parts, identity)].
pub(super) fn scan_refs(
    root_fd: RawFd,
    root_info: &FileStat,
    repo_parts: &[OsString],
    budget: &mut ScanBudget,
    report: &mut CleanupReport,
) -> Result<BTreeMap<String, Vec<(Parts, Identity)>>, JanitorError> {
    let mut by_commit: BTreeMap<String, Vec<(Parts, Identity)>> = BTreeMap::new();
    let mut refs_parts: Parts = repo_parts.to_vec();
    refs_parts.push(OsString::from("refs"));
    let refs_fd = match safefs::open_path(root_fd, &refs_parts) {
        Ok(fd) => fd,
        Err(exc) if exc.kind() == io::ErrorKind::NotFound => return Ok(by_commit),
        Err(exc) => return Err(exc.into()),
    };
    let mut stack: Vec<(Parts, OwnedFd)> = vec![(Vec::new(), refs_fd)];
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
                        return Err(os_error("reference directory changed"));
                    }
                    stack.push((relative, child));
                } else if kind == IFREG {
                    let descriptor = safefs::open_file_at(
                        directory_fd.as_raw_fd(),
                        &name,
                        OFlag::O_RDONLY,
                        Mode::empty(),
                    )?;
                    let payload = {
                        let opened = safefs::fstat(descriptor.as_raw_fd())?;
                        if identity(&opened) != identity(&info) {
                            return Err(os_error("reference changed"));
                        }
                        safefs::read_fd(descriptor.as_raw_fd(), 257)?
                    };
                    drop(descriptor);
                    if payload.len() > 256 {
                        return Err(os_error("oversized cache reference"));
                    }
                    let commit = match std::str::from_utf8(&payload) {
                        Ok(text) if text.is_ascii() => text.trim().to_string(),
                        _ => return Err(os_error("invalid cache reference")),
                    };
                    if commit.is_empty() || !commit.chars().all(|c| c.is_ascii_hexdigit()) {
                        return Err(os_error("invalid cache reference"));
                    }
                    let mut path_parts = refs_parts.clone();
                    path_parts.extend(relative.iter().cloned());
                    by_commit
                        .entry(commit)
                        .or_default()
                        .push((path_parts, identity(&info)));
                } else {
                    return Err(os_error("unsafe cache reference"));
                }
            }
        }
    }
    Ok(by_commit)
}

/// Python `_hf_scan_reserved_metadata`: `.no_exist/` may hold plain dirs
/// and regular files only, never `.work`/`*.incomplete`.
pub(super) fn scan_reserved_metadata(
    root_fd: RawFd,
    root_info: &FileStat,
    parts: &[OsString],
    budget: &mut ScanBudget,
    report: &mut CleanupReport,
) -> Result<(), JanitorError> {
    let metadata_fd = safefs::open_path(root_fd, parts)?;
    let mut stack: Vec<OwnedFd> = vec![metadata_fd];
    while let Some(directory_fd) = stack.pop() {
        {
            for name in safefs::DirEntries::open(directory_fd.as_raw_fd())? {
                let name = name?;
                budget.tick(report)?;
                if name.as_bytes() == b".work" || name.to_string_lossy().ends_with(".incomplete") {
                    return Err(os_error("incomplete reserved cache metadata"));
                }
                let info = safefs::fstatat_nofollow(directory_fd.as_raw_fd(), &name)?;
                check_info(&info, root_info)?;
                if ifmt(info.st_mode as u32) == IFDIR {
                    let child = safefs::open_dir_at(directory_fd.as_raw_fd(), &name)?;
                    if identity(&safefs::fstat(child.as_raw_fd())?) != identity(&info) {
                        return Err(os_error("reserved cache metadata changed"));
                    }
                    stack.push(child);
                } else if ifmt(info.st_mode as u32) != IFREG {
                    return Err(os_error("unsafe reserved cache metadata"));
                }
            }
        }
    }
    Ok(())
}
