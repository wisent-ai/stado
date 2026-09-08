//! The cache inventory: what the HuggingFace cache root holds.
//!
//! [`locks`] walks and flocks the `.locks` namespace, [`repo`] walks one
//! repository, [`snapshots`] one revision of one repository, and [`refs`]
//! the commit pointers plus the reserved `.no_exist/` metadata.
//! `scan_cache` below is the whole-root entry point that dispatches to them.

pub(in crate::providers::local::disk_cleanup::hf) mod locks;
mod refs;
mod repo;
pub(in crate::providers::local::disk_cleanup::hf) mod snapshots;

use std::os::fd::RawFd;

use nix::sys::stat::FileStat;

use crate::providers::local::disk_cleanup::hf::inventory::repo::scan_repo;
use crate::providers::local::disk_cleanup::hf::{check_info, os_error, Parts, RepoScan};
use crate::providers::local::disk_cleanup::{
    ifmt, safefs, CleanupReport, JanitorError, ScanBudget, IFDIR, IFREG,
};

/// Python `_hf_scan_cache`.
pub fn scan_cache(
    root_fd: RawFd,
    root_info: &FileStat,
    budget: &mut ScanBudget,
    report: &mut CleanupReport,
) -> Result<Vec<RepoScan>, JanitorError> {
    let mut repositories: Vec<Parts> = Vec::new();
    let mut direct_layout = false;
    {
        for name in safefs::DirEntries::open(root_fd)? {
            let name = name?;
            budget.tick(report)?;
            let info = safefs::fstatat_nofollow(root_fd, &name)?;
            check_info(&info, root_info)?;
            let text = name.to_string_lossy();
            if name == ".locks" {
                if ifmt(info.st_mode as u32) != IFDIR {
                    return Err(os_error("unsafe lock root"));
                }
            } else if name == "blobs" || name == "refs" || name == "snapshots" {
                direct_layout = true;
            } else if name == "CACHEDIR.TAG" {
                if ifmt(info.st_mode as u32) != IFREG {
                    return Err(os_error("unsafe cache tag"));
                }
            } else if name == "version.txt" {
                if ifmt(info.st_mode as u32) != IFREG {
                    return Err(os_error("unsafe cache version"));
                }
            } else if text.starts_with("models--")
                || text.starts_with("datasets--")
                || text.starts_with("spaces--")
            {
                if ifmt(info.st_mode as u32) != IFDIR {
                    return Err(os_error("unsafe repository root"));
                }
                repositories.push(vec![name]);
            } else {
                return Err(os_error("unknown cache root data"));
            }
        }
    }
    if direct_layout {
        if !repositories.is_empty() {
            return Err(os_error("ambiguous cache layout"));
        }
        repositories.push(Vec::new());
    }
    let mut scans = Vec::new();
    for repo_parts in repositories {
        scans.push(scan_repo(root_fd, root_info, &repo_parts, budget, report)?);
    }
    Ok(scans)
}
