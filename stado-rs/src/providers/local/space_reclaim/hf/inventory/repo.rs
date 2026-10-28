//! One repository: its blobs, its commit pointers, and the snapshot
//! revisions it offers as eviction candidates.

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::{OsStr, OsString};
use std::os::fd::{AsRawFd, RawFd};
use std::os::unix::ffi::OsStrExt;

use nix::sys::stat::FileStat;

use crate::providers::local::space_reclaim::hf::inventory::refs::{
    scan_refs, scan_reserved_metadata,
};
use crate::providers::local::space_reclaim::hf::inventory::snapshots::snapshot_state;
use crate::providers::local::space_reclaim::hf::{
    check_info, identity, os_error, HfCandidate, Identity, Parts, RepoScan,
};
use crate::providers::local::space_reclaim::{
    ifmt, safefs, CleanupReport, JanitorError, ScanCount, IFDIR, IFREG,
};

/// stat(2): `st_blocks` counts 512-byte units, whatever the filesystem's
/// own block size (https://man7.org/linux/man-pages/man3/stat.3type.html).
const STAT_BLOCK_BYTES: i64 = 512;
/// stat(2): a file with exactly one directory entry has a link count of
/// one, so deleting that entry frees it
/// (https://man7.org/linux/man-pages/man3/stat.3type.html).
const SOLE_LINK: u64 = 1;

/// Python `_hf_scan_repo`. `repo_parts` is empty for the direct layout.
pub(super) fn scan_repo(
    root_fd: RawFd,
    root_info: &FileStat,
    repo_parts: &[OsString],
    budget: &mut ScanCount,
    report: &mut CleanupReport,
) -> Result<RepoScan, JanitorError> {
    let empty = RepoScan {
        candidates: Vec::new(),
        blobs: BTreeMap::new(),
        blob_sizes: BTreeMap::new(),
    };
    let repo_fd = if repo_parts.is_empty() {
        safefs::dup_fd(root_fd)?
    } else {
        safefs::open_path(root_fd, repo_parts)?
    };
    let mut names: BTreeSet<OsString> = BTreeSet::new();
    let mut has_no_exist = false;
    {
        for name in safefs::DirEntries::open(repo_fd.as_raw_fd())? {
            let name = name?;
            budget.tick(report)?;
            let info = safefs::fstatat_nofollow(repo_fd.as_raw_fd(), &name)?;
            check_info(&info, root_info)?;
            if name.as_bytes() == b".work" || name.to_string_lossy().ends_with(".incomplete") {
                return Err(os_error("reserved repository data"));
            }
            if repo_parts.is_empty() && matches!(name.to_str(), Some(".locks" | "version.txt")) {
                continue;
            }
            if name == ".no_exist" {
                if ifmt(info.st_mode as u32) != IFDIR {
                    return Err(os_error("unsafe reserved repository metadata"));
                }
                has_no_exist = true;
                continue;
            }
            names.insert(name.clone());
            if !matches!(name.to_str(), Some("blobs" | "refs" | "snapshots")) {
                return Err(os_error("unknown repository data"));
            }
            if ifmt(info.st_mode as u32) != IFDIR {
                return Err(os_error("unsafe repository layout"));
            }
        }
    }
    drop(repo_fd);
    if has_no_exist {
        let mut metadata_parts = repo_parts.to_vec();
        metadata_parts.push(OsString::from(".no_exist"));
        scan_reserved_metadata(root_fd, root_info, &metadata_parts, budget, report)?;
    }
    // This repository, counted once in the skip tally.
    let this_repository = std::iter::once(repo_parts).count() as i64;
    if !(names.contains(OsStr::new("blobs")) && names.contains(OsStr::new("snapshots"))) {
        report.skip_hf("incomplete_repository", this_repository);
        return Ok(empty);
    }
    let mut blobs_parts = repo_parts.to_vec();
    blobs_parts.push(OsString::from("blobs"));
    let blobs_fd = safefs::open_path(root_fd, &blobs_parts)?;
    let mut blobs: BTreeMap<Parts, Identity> = BTreeMap::new();
    let mut blob_sizes: BTreeMap<Parts, i64> = BTreeMap::new();
    let mut has_incomplete_blob = false;
    {
        for name in safefs::DirEntries::open(blobs_fd.as_raw_fd())? {
            let name = name?;
            budget.tick(report)?;
            let info = safefs::fstatat_nofollow(blobs_fd.as_raw_fd(), &name)?;
            check_info(&info, root_info)?;
            if name.to_string_lossy().ends_with(".incomplete") || name.as_bytes() == b".work" {
                let expected_type = if name.as_bytes() == b".work" {
                    IFDIR
                } else {
                    IFREG
                };
                if ifmt(info.st_mode as u32) != expected_type {
                    return Err(os_error("unsafe incomplete blob data"));
                }
                has_incomplete_blob = true;
                continue;
            }
            if ifmt(info.st_mode as u32) != IFREG {
                return Err(os_error("unsafe blob data"));
            }
            let mut blob_parts = blobs_parts.clone();
            blob_parts.push(name.clone());
            blobs.insert(blob_parts.clone(), identity(&info));
            blob_sizes.insert(
                blob_parts,
                info.st_size.max(info.st_blocks * STAT_BLOCK_BYTES),
            );
        }
    }
    drop(blobs_fd);
    if has_incomplete_blob {
        report.skip_hf("incomplete_repository", this_repository);
        return Ok(empty);
    }
    let refs = scan_refs(root_fd, root_info, repo_parts, budget, report)?;
    let mut snapshots_parts = repo_parts.to_vec();
    snapshots_parts.push(OsString::from("snapshots"));
    let snapshots_fd = safefs::open_path(root_fd, &snapshots_parts)?;
    let mut candidates: Vec<HfCandidate> = Vec::new();
    {
        for name in safefs::DirEntries::open(snapshots_fd.as_raw_fd())? {
            let name = name?;
            budget.tick(report)?;
            let info = safefs::fstatat_nofollow(snapshots_fd.as_raw_fd(), &name)?;
            check_info(&info, root_info)?;
            if ifmt(info.st_mode as u32) != IFDIR {
                return Err(os_error("unsafe snapshot revision"));
            }
            let (state, modified, snapshot_expected, referenced_blobs) = snapshot_state(
                root_fd, root_info, repo_parts, &name, &blobs, budget, report,
            )?;
            let commit_key = name.to_string_lossy().into_owned();
            candidates.push(HfCandidate {
                repo: repo_parts.to_vec(),
                commit: name,
                snapshot: state,
                modified,
                snapshot_expected,
                expected: snapshot_expected,
                referenced_blobs,
                delete_blobs: Vec::new(),
                // A snapshot no ref names has no refs to recheck.
                refs: match refs.get(&commit_key) {
                    Some(named) => named.clone(),
                    None => Vec::new(),
                },
                deleted: false,
            });
        }
    }
    drop(snapshots_fd);
    let indices: Vec<usize> = candidates
        .iter()
        .enumerate()
        .map(|(index, _)| index)
        .collect();
    for index in indices {
        // Every blob another candidate still references stays.
        let retained_references: BTreeSet<Parts> = candidates
            .iter()
            .enumerate()
            .filter(|(other, _)| *other != index)
            .flat_map(|(_, other)| other.referenced_blobs.iter().cloned())
            .collect();
        let exclusive: BTreeSet<Parts> = candidates[index]
            .referenced_blobs
            .difference(&retained_references)
            .cloned()
            .collect();
        let unique: BTreeSet<Parts> = exclusive
            .iter()
            .filter(|path| blobs[*path].nlink == SOLE_LINK)
            .cloned()
            .collect();
        if unique.len() != exclusive.len() {
            report.skip_hf(
                "blob_link_count_uncertain",
                (exclusive.len() - unique.len()) as i64,
            );
        }
        candidates[index].delete_blobs = unique
            .iter()
            .map(|path| (path.clone(), blobs[path]))
            .collect();
        candidates[index].expected += unique.iter().map(|path| blob_sizes[path]).sum::<i64>();
    }
    Ok(RepoScan {
        candidates,
        blobs,
        blob_sizes,
    })
}
