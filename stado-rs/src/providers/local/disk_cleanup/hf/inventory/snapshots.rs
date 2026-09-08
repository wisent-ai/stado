//! One snapshot revision: the link normalizer and the revision scan.

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::{OsStr, OsString};
use std::os::fd::{AsRawFd, OwnedFd, RawFd};
use std::os::unix::ffi::OsStrExt;

use nix::sys::stat::FileStat;

use crate::providers::local::disk_cleanup::hf::{
    check_info, identity, os_error, Identity, Parts, SnapshotScan,
};
use crate::providers::local::disk_cleanup::{
    safefs, CleanupReport, JanitorError, ScanBudget, IFDIR, IFLNK, IFREG,
};

/// Python `_hf_normalize_link`: resolve a snapshot symlink target lexically
/// and require it to name a blob of the SAME repository. Anything else
/// (absolute target, escape above the snapshot parent, target outside
/// `<repo>/blobs/`) refuses the whole snapshot.
fn normalize_link(
    repo_parts: &[OsString],
    link_parts: &[OsString],
    target: &OsStr,
) -> Result<Parts, JanitorError> {
    let target_str = target.to_string_lossy();
    if target_str.is_empty() || target_str.starts_with('/') {
        return Err(os_error("unsafe snapshot link"));
    }
    let mut parts: Vec<OsString> = link_parts[..link_parts.len() - 1].to_vec();
    for part in target_str.split('/') {
        if part.is_empty() || part == "." {
            continue;
        }
        if part == ".." {
            if parts.is_empty() {
                return Err(os_error("snapshot link escapes cache"));
            }
            parts.pop();
        } else {
            parts.push(OsString::from(part));
        }
    }
    let mut expected: Parts = repo_parts.to_vec();
    expected.push(OsString::from("blobs"));
    if parts.len() < expected.len() || parts[..expected.len()] != expected[..] {
        return Err(os_error("snapshot link does not target repository blob"));
    }
    Ok(parts)
}

/// Python `_hf_leaf_info`.
fn leaf_info(root_fd: RawFd, parts: &[OsString]) -> Result<FileStat, JanitorError> {
    if parts.is_empty() {
        return Ok(safefs::fstat(root_fd)?);
    }
    let parent_fd = safefs::open_path(root_fd, &parts[..parts.len() - 1])?;
    let info = safefs::fstatat_nofollow(parent_fd.as_raw_fd(), &parts[parts.len() - 1]);
    drop(parent_fd);
    Ok(info?)
}

/// Python `_hf_snapshot_state`. Returns (state, modified, expected_bytes,
/// referenced_blobs).
#[allow(clippy::too_many_arguments)]
pub(in crate::providers::local::disk_cleanup::hf) fn snapshot_state(
    root_fd: RawFd,
    root_info: &FileStat,
    repo_parts: &[OsString],
    commit: &OsStr,
    blobs: &BTreeMap<Parts, Identity>,
    budget: &mut ScanBudget,
    report: &mut CleanupReport,
) -> Result<SnapshotScan, JanitorError> {
    let mut snapshot_parts: Parts = repo_parts.to_vec();
    snapshot_parts.push(OsString::from("snapshots"));
    snapshot_parts.push(commit.to_os_string());
    let snapshot_fd = safefs::open_path(root_fd, &snapshot_parts)?;
    let snapshot_info = safefs::fstat(snapshot_fd.as_raw_fd())?;
    check_info(&snapshot_info, root_info)?;
    let mut state: BTreeMap<Parts, Identity> = BTreeMap::new();
    state.insert(Vec::new(), identity(&snapshot_info));
    let mut modified = snapshot_info.st_mtime as f64;
    let mut expected = snapshot_info.st_blocks * 512;
    let mut referenced_blobs: BTreeSet<Parts> = BTreeSet::new();
    let mut stack: Vec<(Parts, OwnedFd)> = vec![(Vec::new(), snapshot_fd)];
    while let Some((prefix, directory_fd)) = stack.pop() {
        {
            for name in safefs::DirEntries::open(directory_fd.as_raw_fd())? {
                let name = name?;
                budget.tick(report)?;
                if name == "." || name == ".." {
                    continue;
                }
                if name.as_bytes() == b".work" || name.to_string_lossy().ends_with(".incomplete") {
                    return Err(os_error("reserved snapshot data"));
                }
                let info = safefs::fstatat_nofollow(directory_fd.as_raw_fd(), &name)?;
                check_info(&info, root_info)?;
                let mut relative = prefix.clone();
                relative.push(name.clone());
                let ident = identity(&info);
                state.insert(relative.clone(), ident);
                modified = modified.max(info.st_mtime as f64);
                let kind = ident.ifmt;
                if kind == IFDIR {
                    let child = safefs::open_dir_at(directory_fd.as_raw_fd(), &name)?;
                    if identity(&safefs::fstat(child.as_raw_fd())?) != ident {
                        return Err(os_error("snapshot directory changed"));
                    }
                    expected += info.st_blocks * 512;
                    stack.push((relative, child));
                    continue;
                }
                if kind == IFLNK {
                    let target = safefs::readlink_at(directory_fd.as_raw_fd(), &name)?;
                    let mut full = snapshot_parts.clone();
                    full.extend(relative.iter().cloned());
                    let blob_parts = normalize_link(repo_parts, &full, &target)?;
                    let Some(blob_identity) = blobs.get(&blob_parts) else {
                        return Err(os_error("snapshot references unknown blob"));
                    };
                    let current_blob = leaf_info(root_fd, &blob_parts)?;
                    if identity(&current_blob) != *blob_identity {
                        return Err(os_error("snapshot blob changed"));
                    }
                    referenced_blobs.insert(blob_parts);
                    modified = modified.max(blob_identity.mtime_ns as f64 / 1_000_000_000.0);
                    expected += info.st_blocks * 512;
                    continue;
                }
                if kind == IFREG {
                    let matches: Vec<&Parts> = blobs
                        .iter()
                        .filter(|(_, blob_identity)| {
                            blob_identity.dev == ident.dev && blob_identity.ino == ident.ino
                        })
                        .map(|(path, _)| path)
                        .collect();
                    if matches.len() != 1 {
                        return Err(os_error("untracked or ambiguous snapshot data"));
                    }
                    referenced_blobs.insert(matches[0].clone());
                    continue;
                }
                return Err(os_error("unsupported snapshot entry"));
            }
        }
    }
    if state.len() == 1 {
        return Err(os_error("empty snapshot"));
    }
    Ok((state, modified, expected, referenced_blobs))
}
