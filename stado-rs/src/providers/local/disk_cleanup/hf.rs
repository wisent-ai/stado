//! HuggingFace cache eviction: the sandboxed scan, the `.locks` flock
//! acquisition, the atomic rename-exchange lock-barrier dance, and the
//! identity-checked deletion pass.
//!
//! Port of the HF half of `stado/providers/local/disk/cleanup.py`
//! (`_hf_*` helpers + `_run_hf`). The Python validates huggingface_hub
//! SDK deletion plans (`_validate_hf_strategy`); those entry points are
//! dead code in Python (never called — the scan below replaced the SDK)
//! and are NOT ported. Instead this module reimplements the HF cache
//! layout scan directly over `blobs/`, `refs/`, `snapshots/`, `.locks/`,
//! proving — like the Python — that only tracked cache data of the
//! selected revision is ever unlinked.
//!
//! Every Python safety comment is preserved at its Rust site. All
//! operations are dir_fd-relative (see [`super::safefs`]): after the root
//! is validated once, no absolute path is ever dereferenced again.

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::{OsStr, OsString};
use std::fs::File;
use std::io;
use std::os::fd::{AsRawFd, OwnedFd, RawFd};
use std::os::unix::ffi::OsStrExt;
use std::path::Path;
use std::time::Instant;

use nix::fcntl::OFlag;
use nix::sys::stat::{FileStat, Mode};

use super::safefs;
use super::{
    euid, fixed_root, free_bytes, ifmt, CleanupReport, JanitorError, ScanBudget, IFDIR, IFLNK,
    IFREG,
};
use crate::targets::DiskCleanupPolicy;

/// Python `_HF_BARRIER_NAME`.
const HF_BARRIER_NAME: &str = ".wisent-compute-lock-barrier";
/// Python `_HF_BARRIER_MARKER`.
const HF_BARRIER_MARKER: &str = ".wisent-compute-barrier";

/// Path components beneath the cache root (Python `tuple[str, ...]`).
pub type Parts = Vec<OsString>;

/// Stable identity triple (Python `_hf_stable_identity`):
/// (st_dev, st_ino, S_IFMT(mode)).
type StableId = (u64, u64, u32);

/// The lock-namespace scan result: path-parts -> identity map, the held
/// lock files, and whether the lock root exists at all.
type LockScan = (BTreeMap<Parts, Identity>, Vec<File>, bool);

/// One snapshot's scan result: state map, max mtime (epoch seconds),
/// expected reclaimable bytes, referenced blobs.
type SnapshotScan = (BTreeMap<Parts, Identity>, f64, i64, BTreeSet<Parts>);

/// Full identity tuple (Python `_hf_identity`):
/// (st_dev, st_ino, S_IFMT(mode), st_size, st_mtime_ns, st_nlink).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Identity {
    pub dev: u64,
    pub ino: u64,
    pub ifmt: u32,
    pub size: i64,
    pub mtime_ns: i64,
    pub nlink: u64,
}

impl Identity {
    /// Python `_hf_stable_identity`: (st_dev, st_ino, S_IFMT(mode)).
    fn stable(&self) -> StableId {
        (self.dev, self.ino, self.ifmt)
    }
}

#[allow(clippy::unnecessary_cast)] // libc field widths differ per OS; the cast is required on macOS
fn identity(info: &FileStat) -> Identity {
    Identity {
        dev: info.st_dev as u64,
        ino: info.st_ino,
        ifmt: ifmt(info.st_mode as u32),
        size: info.st_size,
        mtime_ns: info.st_mtime * 1_000_000_000 + info.st_mtime_nsec,
        nlink: info.st_nlink as u64,
    }
}

fn stable_identity(info: &FileStat) -> StableId {
    identity(info).stable()
}

fn os_error(message: &str) -> JanitorError {
    JanitorError::os(message)
}

/// Python `_hf_check_info`: every cache entry must be owned by the cleaner's
/// euid and live on the same device as the cache root.
fn check_info(info: &FileStat, root_info: &FileStat) -> Result<(), JanitorError> {
    if info.st_uid != euid() || info.st_dev != root_info.st_dev {
        return Err(os_error("cache entry ownership or device mismatch"));
    }
    Ok(())
}

/// One scanned snapshot revision plus everything needed to delete it.
/// (Python's per-candidate dict.)
#[derive(Debug, Clone)]
pub struct HfCandidate {
    pub repo: Parts,
    pub commit: OsString,
    /// Relative-parts -> identity for every entry in the snapshot
    /// (`()` = the snapshot root itself).
    pub snapshot: BTreeMap<Parts, Identity>,
    pub modified: f64,
    pub snapshot_expected: i64,
    pub expected: i64,
    pub referenced_blobs: BTreeSet<Parts>,
    pub delete_blobs: Vec<(Parts, Identity)>,
    pub refs: Vec<(Parts, Identity)>,
    pub deleted: bool,
}

/// One scanned repository (the shared blob state Python threads through
/// `candidate["blobs"]` / `candidate["repo_candidates"]`).
#[derive(Debug)]
pub struct RepoScan {
    pub candidates: Vec<HfCandidate>,
    pub blobs: BTreeMap<Parts, Identity>,
    pub blob_sizes: BTreeMap<Parts, i64>,
}

// ---------------------------------------------------------------------------
// .locks acquisition (Python `_hf_lock_state`)
// ---------------------------------------------------------------------------

/// Walk the `.locks` tree, recording identities and (when `acquire`) taking
/// an exclusive flock on every regular lock file. Returns
/// (state, held lock files, present).
///
/// Python raises `BlockingIOError("cache lock held")` when any lock is
/// already held by a live download — the caller turns that into the
/// `cache_locked` skip and touches nothing.
fn scan_lock_state(
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
                            Err(exc) if super::lock_contended(&exc) => {
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

// ---------------------------------------------------------------------------
// lock-barrier dance (Python `_hf_*lock_barrier*` + `_hf_exchange`)
// ---------------------------------------------------------------------------

/// Python `_hf_has_barrier_marker`.
fn has_barrier_marker(directory_fd: RawFd, root_info: &FileStat) -> Result<bool, JanitorError> {
    let info = match safefs::fstatat_nofollow(directory_fd, OsStr::new(HF_BARRIER_MARKER)) {
        Ok(info) => info,
        Err(exc) if exc.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(exc) => return Err(exc.into()),
    };
    check_info(&info, root_info)?;
    if ifmt(info.st_mode as u32) != IFREG {
        return Err(os_error("unsafe cache lock barrier marker"));
    }
    Ok(true)
}

/// Python `_hf_remove_barrier_tree`: delete the barrier copy of the lock
/// namespace. Every entry is re-validated (owner, device, plain dir/file)
/// before it is unlinked.
fn remove_barrier_tree(directory_fd: RawFd, root_info: &FileStat) -> Result<(), JanitorError> {
    safefs::fchmod(directory_fd, Mode::from_bits_truncate(0o700))?;
    let mut names = Vec::new();
    {
        let entries = safefs::DirEntries::open(directory_fd)?;
        for name in entries {
            names.push(name?);
        }
    }
    for name in names {
        let info = safefs::fstatat_nofollow(directory_fd, &name)?;
        check_info(&info, root_info)?;
        let kind = ifmt(info.st_mode as u32);
        if kind == IFDIR {
            let child = safefs::open_dir_at(directory_fd, &name)?;
            if stable_identity(&safefs::fstat(child.as_raw_fd())?) != stable_identity(&info) {
                return Err(os_error("cache lock barrier directory changed"));
            }
            remove_barrier_tree(child.as_raw_fd(), root_info)?;
            drop(child);
            safefs::rmdir_at(directory_fd, &name)?;
        } else if kind == IFREG {
            safefs::unlink_at(directory_fd, &name)?;
        } else {
            return Err(os_error("unsafe cache lock barrier entry"));
        }
    }
    Ok(())
}

/// Python `_hf_discard_barrier`.
fn discard_barrier(root_fd: RawFd, root_info: &FileStat) -> Result<(), JanitorError> {
    let barrier_fd = safefs::open_dir_at(root_fd, OsStr::new(HF_BARRIER_NAME))?;
    let result = remove_barrier_tree(barrier_fd.as_raw_fd(), root_info);
    drop(barrier_fd);
    result?;
    safefs::rmdir_at(root_fd, OsStr::new(HF_BARRIER_NAME))?;
    Ok(())
}

/// Restore an atomic exchange interrupted by process termination.
/// Python `_hf_recover_lock_barrier`.
pub fn recover_lock_barrier(root_fd: RawFd, root_info: &FileStat) -> Result<(), JanitorError> {
    let barrier_fd = match safefs::open_dir_at(root_fd, OsStr::new(HF_BARRIER_NAME)) {
        Ok(fd) => fd,
        Err(exc) if exc.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(exc) => return Err(exc.into()),
    };
    let private_marked = {
        check_info(&safefs::fstat(barrier_fd.as_raw_fd())?, root_info)
            .and_then(|()| has_barrier_marker(barrier_fd.as_raw_fd(), root_info))
    };
    drop(barrier_fd);
    let private_marked = private_marked?;
    let locks_fd = safefs::open_dir_at(root_fd, OsStr::new(".locks"))?;
    let canonical_marked = {
        check_info(&safefs::fstat(locks_fd.as_raw_fd())?, root_info)
            .and_then(|()| has_barrier_marker(locks_fd.as_raw_fd(), root_info))
    };
    drop(locks_fd);
    let canonical_marked = canonical_marked?;
    if canonical_marked && !private_marked {
        safefs::rename_exchange(root_fd, OsStr::new(".locks"), OsStr::new(HF_BARRIER_NAME))?;
        discard_barrier(root_fd, root_info)?;
    } else if private_marked && !canonical_marked {
        discard_barrier(root_fd, root_info)?;
    } else {
        return Err(os_error("ambiguous cache lock barrier residue"));
    }
    Ok(())
}

/// Python `_hf_prepare_lock_barrier`: build a private hard-linked copy of
/// the whole `.locks` namespace so the atomic exchange never destroys a
/// lock another process holds.
fn prepare_lock_barrier(
    root_fd: RawFd,
    root_info: &FileStat,
    lock_state: &BTreeMap<Parts, Identity>,
) -> Result<StableId, JanitorError> {
    safefs::mkdir_at(
        root_fd,
        OsStr::new(HF_BARRIER_NAME),
        Mode::from_bits_truncate(0o700),
    )?;
    let barrier_fd = safefs::open_dir_at(root_fd, OsStr::new(HF_BARRIER_NAME))?;
    let result = (|barrier_fd: &OwnedFd| {
        let marker = safefs::open_file_at(
            barrier_fd.as_raw_fd(),
            OsStr::new(HF_BARRIER_MARKER),
            OFlag::O_WRONLY | OFlag::O_CREAT | OFlag::O_EXCL,
            Mode::from_bits_truncate(0o400),
        )?;
        drop(marker);
        let mut directories: Vec<&Parts> = lock_state
            .iter()
            .filter(|(parts, ident)| !parts.is_empty() && ident.ifmt == IFDIR)
            .map(|(parts, _)| parts)
            .collect();
        directories.sort_by_key(|parts| parts.len());
        for parts in &directories {
            let parent = safefs::open_path(barrier_fd.as_raw_fd(), &parts[..parts.len() - 1])?;
            safefs::mkdir_at(
                parent.as_raw_fd(),
                &parts[parts.len() - 1],
                Mode::from_bits_truncate(0o700),
            )?;
        }
        for (parts, ident) in lock_state {
            if parts.is_empty() || ident.ifmt != IFREG {
                continue;
            }
            let mut source_parts: Parts = vec![OsString::from(".locks")];
            source_parts.extend_from_slice(&parts[..parts.len() - 1]);
            let source_parent = safefs::open_path(root_fd, &source_parts)?;
            let destination_parent =
                safefs::open_path(barrier_fd.as_raw_fd(), &parts[..parts.len() - 1])?;
            let expected_parent = &lock_state[&parts[..parts.len() - 1].to_vec()];
            if stable_identity(&safefs::fstat(source_parent.as_raw_fd())?)
                != expected_parent.stable()
            {
                return Err(os_error("cache lock parent changed while building barrier"));
            }
            safefs::link_at(
                source_parent.as_raw_fd(),
                destination_parent.as_raw_fd(),
                &parts[parts.len() - 1],
            )?;
            let linked =
                safefs::fstatat_nofollow(destination_parent.as_raw_fd(), &parts[parts.len() - 1])?;
            if stable_identity(&linked) != ident.stable() {
                return Err(os_error("cache lock changed while building barrier"));
            }
        }
        for parts in directories.iter().rev() {
            let descriptor = safefs::open_path(barrier_fd.as_raw_fd(), parts)?;
            safefs::fchmod(descriptor.as_raw_fd(), Mode::from_bits_truncate(0o555))?;
        }
        safefs::fchmod(barrier_fd.as_raw_fd(), Mode::from_bits_truncate(0o555))?;
        Ok(stable_identity(&safefs::fstat(barrier_fd.as_raw_fd())?))
    })(&barrier_fd);
    drop(barrier_fd);
    match result {
        Ok(identity) => Ok(identity),
        Err(exc) => {
            discard_barrier(root_fd, root_info)?;
            Err(exc)
        }
    }
}

/// Python `_hf_barrier_lock_state_matches`: after the exchange the private
/// namespace must equal the recorded state except for the +1 link count
/// the barrier hard links added to every regular file.
fn barrier_lock_state_matches(
    expected: &BTreeMap<Parts, Identity>,
    current: &BTreeMap<Parts, Identity>,
) -> bool {
    if expected.len() != current.len() || !expected.keys().zip(current.keys()).all(|(a, b)| a == b)
    {
        return false;
    }
    for (path, ident) in expected {
        let observed = &current[path];
        if ident.ifmt == IFREG {
            let matches = observed.dev == ident.dev
                && observed.ino == ident.ino
                && observed.ifmt == ident.ifmt
                && observed.size == ident.size
                && observed.mtime_ns == ident.mtime_ns
                && observed.nlink == ident.nlink + 1;
            if !matches {
                return false;
            }
        } else if observed != ident {
            return false;
        }
    }
    true
}

/// Python `_hf_enter_lock_barrier`. Returns (original, barrier) stable
/// identities used by [`leave_lock_barrier`].
fn enter_lock_barrier(
    root_fd: RawFd,
    root_info: &FileStat,
    lock_state: &BTreeMap<Parts, Identity>,
    lock_fds: &[File],
    budget: &mut ScanBudget,
    report: &mut CleanupReport,
) -> Result<(StableId, StableId), JanitorError> {
    let root_key: Parts = Vec::new();
    let original = lock_state[&root_key].stable();
    let barrier = prepare_lock_barrier(root_fd, root_info, lock_state)?;
    let mut exchanged = false;
    let result = (|exchanged: &mut bool| {
        safefs::rename_exchange(root_fd, OsStr::new(".locks"), OsStr::new(HF_BARRIER_NAME))?;
        *exchanged = true;
        let canonical_fd = safefs::open_dir_at(root_fd, OsStr::new(".locks"))?;
        let private_fd = safefs::open_dir_at(root_fd, OsStr::new(HF_BARRIER_NAME))?;
        {
            if stable_identity(&safefs::fstat(canonical_fd.as_raw_fd())?) != barrier {
                return Err(os_error("cache lock barrier exchange changed"));
            }
            if stable_identity(&safefs::fstat(private_fd.as_raw_fd())?) != original {
                return Err(os_error("cache lock namespace changed during exchange"));
            }
        }
        drop(canonical_fd);
        drop(private_fd);
        let held_identities: BTreeSet<StableId> = lock_fds
            .iter()
            .map(|fd| safefs::fstat(fd.as_raw_fd()).map(|i| stable_identity(&i)))
            .collect::<io::Result<BTreeSet<_>>>()?;
        let expected_held: BTreeSet<StableId> = lock_state
            .values()
            .filter(|ident| ident.ifmt == IFREG)
            .map(Identity::stable)
            .collect();
        let (current, new_fds, present) = scan_lock_state(
            root_fd,
            root_info,
            budget,
            report,
            true,
            HF_BARRIER_NAME,
            &held_identities,
        )?;
        let check = (|current: &BTreeMap<Parts, Identity>| {
            if !present || !barrier_lock_state_matches(lock_state, current) {
                return Err(os_error("cache lock set changed during barrier exchange"));
            }
            if held_identities != expected_held {
                return Err(os_error("held cache lock changed during barrier exchange"));
            }
            Ok(())
        })(&current);
        drop(new_fds);
        check?;
        Ok((original, barrier))
    })(&mut exchanged);
    match result {
        Ok(identities) => Ok(identities),
        Err(exc) => {
            if exchanged {
                safefs::rename_exchange(
                    root_fd,
                    OsStr::new(".locks"),
                    OsStr::new(HF_BARRIER_NAME),
                )?;
            }
            discard_barrier(root_fd, root_info)?;
            Err(exc)
        }
    }
}

/// Python `_hf_leave_lock_barrier`: exchange back, verify restoration,
/// discard the private namespace.
fn leave_lock_barrier(
    root_fd: RawFd,
    root_info: &FileStat,
    identities: (StableId, StableId),
) -> Result<(), JanitorError> {
    let (original, barrier) = identities;
    let canonical_fd = safefs::open_dir_at(root_fd, OsStr::new(".locks"))?;
    let private_fd = safefs::open_dir_at(root_fd, OsStr::new(HF_BARRIER_NAME))?;
    {
        if stable_identity(&safefs::fstat(canonical_fd.as_raw_fd())?) != barrier {
            return Err(os_error("cache lock barrier changed before restoration"));
        }
        if stable_identity(&safefs::fstat(private_fd.as_raw_fd())?) != original {
            return Err(os_error(
                "held cache lock namespace changed before restoration",
            ));
        }
    }
    drop(canonical_fd);
    drop(private_fd);
    safefs::rename_exchange(root_fd, OsStr::new(".locks"), OsStr::new(HF_BARRIER_NAME))?;
    let restored_fd = safefs::open_dir_at(root_fd, OsStr::new(".locks"))?;
    {
        if stable_identity(&safefs::fstat(restored_fd.as_raw_fd())?) != original {
            return Err(os_error("cache lock namespace restoration failed"));
        }
    }
    drop(restored_fd);
    discard_barrier(root_fd, root_info)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// snapshot + refs scan
// ---------------------------------------------------------------------------

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
fn snapshot_state(
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

/// Python `_hf_scan_refs`: commit -> [(ref path parts, identity)].
fn scan_refs(
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
fn scan_reserved_metadata(
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

/// Python `_hf_scan_repo`. `repo_parts` is empty for the direct layout.
fn scan_repo(
    root_fd: RawFd,
    root_info: &FileStat,
    repo_parts: &[OsString],
    budget: &mut ScanBudget,
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
            if repo_parts.is_empty() && (name == ".locks" || name == "version.txt") {
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
            if !(name == "blobs" || name == "refs" || name == "snapshots") {
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
    if !(names.contains(OsStr::new("blobs")) && names.contains(OsStr::new("snapshots"))) {
        report.skip_hf("incomplete_repository", 1);
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
            blob_sizes.insert(blob_parts, info.st_size.max(info.st_blocks * 512));
        }
    }
    drop(blobs_fd);
    if has_incomplete_blob {
        report.skip_hf("incomplete_repository", 1);
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
                refs: refs.get(&commit_key).cloned().unwrap_or_default(),
                deleted: false,
            });
        }
    }
    drop(snapshots_fd);
    for index in 0..candidates.len() {
        let retained_references: BTreeSet<Parts> = if candidates.len() > 1 {
            candidates
                .iter()
                .enumerate()
                .filter(|(other, _)| *other != index)
                .flat_map(|(_, other)| other.referenced_blobs.iter().cloned())
                .collect()
        } else {
            BTreeSet::new()
        };
        let exclusive: BTreeSet<Parts> = candidates[index]
            .referenced_blobs
            .difference(&retained_references)
            .cloned()
            .collect();
        let unique: BTreeSet<Parts> = exclusive
            .iter()
            .filter(|path| blobs[*path].nlink == 1)
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

// ---------------------------------------------------------------------------
// pre-deletion rechecks + identity-checked unlink
// ---------------------------------------------------------------------------

/// Python `_hf_recheck_ref`.
fn recheck_ref(
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
fn recheck_repository_snapshots(
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
fn execute_candidate(root_fd: RawFd, candidate: &HfCandidate) -> Result<(), JanitorError> {
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

// ---------------------------------------------------------------------------
// the cleaner entry point (Python `_run_hf`)
// ---------------------------------------------------------------------------

const GIB: i64 = 1024 * 1024 * 1024;

/// Run one bounded HF eviction pass. Returns (deleted, expected_total).
/// Python `_run_hf`. Scan-phase failures land in the report as skips or
/// bounded errors (and yield `Ok((0, 0))`); only failures Python lets
/// ESCAPE `_run_hf` (a vanished cache root mid-pass, a failed free-space
/// probe) are returned as `Err`.
pub fn run_hf(
    home: &Path,
    policy: &DiskCleanupPolicy,
    active_slot_count: i64,
    now: f64,
    deadline: Instant,
    report: &mut CleanupReport,
) -> Result<(i64, i64), JanitorError> {
    let Some(configured) = policy.cleaners.get("huggingface_cache") else {
        return Ok((0, 0));
    };
    if active_slot_count > 0 {
        report.skip_hf("active_slots", 1);
        return Ok((0, 0));
    }

    let mut budget = ScanBudget::new(policy.max_scan_items, deadline);
    let scan_phase = (|budget: &mut ScanBudget, report: &mut CleanupReport| {
        let parts = [
            OsString::from(".cache"),
            OsString::from("huggingface"),
            OsString::from("hub"),
        ];
        let Some(root) = fixed_root(home, &parts, false)? else {
            report.skip_hf("root_absent", 1);
            return Ok(None);
        };
        let root_fd = safefs::open_dir_path(&root)?;
        let mut root_info = safefs::fstat(root_fd.as_raw_fd())?;
        check_info(&root_info, &root_info)?;
        let path_info = std::fs::metadata(&root).map_err(JanitorError::from)?;
        if identity(&root_info) != identity_from_metadata(&path_info) {
            return Err(os_error("cache root changed while opening"));
        }
        if policy.mode == "enforce" {
            recover_lock_barrier(root_fd.as_raw_fd(), &root_info)?;
            root_info = safefs::fstat(root_fd.as_raw_fd())?;
            let path_info = std::fs::metadata(&root).map_err(JanitorError::from)?;
            if identity(&root_info) != identity_from_metadata(&path_info) {
                return Err(os_error("cache root changed during lock barrier recovery"));
            }
        }
        let (lock_state_map, lock_fds, locks_present) = match scan_lock_state(
            root_fd.as_raw_fd(),
            &root_info,
            budget,
            report,
            true,
            ".locks",
            &BTreeSet::new(),
        ) {
            Ok(value) => value,
            Err(exc) if exc.code == "BlockingIOError" => {
                report.skip_hf("cache_locked", 1);
                return Ok(None);
            }
            Err(exc) => return Err(exc),
        };
        let scans = scan_cache(root_fd.as_raw_fd(), &root_info, budget, report)?;
        Ok(Some((
            root_fd,
            root_info,
            lock_state_map,
            lock_fds,
            locks_present,
            scans,
        )))
    })(&mut budget, report);

    let (root_fd, mut root_info, lock_state_map, lock_fds, locks_present, scans) = match scan_phase
    {
        Ok(Some(value)) => value,
        Ok(None) => return Ok((0, 0)),
        Err(exc) => {
            if report.caps.deadline {
                report.skip_hf("scan_deadline", 1);
            } else if report.caps.scan {
                report.skip_hf("scan_cap", 1);
            } else {
                report.add_error("huggingface_cache", &exc);
            }
            return Ok((0, 0));
        }
    };
    // root_fd / lock_fds are RAII guards: closing them (Python's
    // try/finally os.close) happens when this pass returns.

    let mut candidates: Vec<(usize, usize)> = Vec::new(); // (scan index, candidate index)
    for (scan_index, scan) in scans.iter().enumerate() {
        for (candidate_index, candidate) in scan.candidates.iter().enumerate() {
            if candidate.modified <= now - configured.min_age_seconds as f64 {
                candidates.push((scan_index, candidate_index));
            }
        }
    }
    let young = scans
        .iter()
        .flat_map(|scan| scan.candidates.iter())
        .filter(|candidate| candidate.modified > now - configured.min_age_seconds as f64)
        .count();
    if young > 0 {
        report.skip_hf("too_young", young as i64);
    }
    report.hf.eligible_items = candidates.len() as i64;
    candidates.sort_by(|a, b| {
        let ca = &scans[a.0].candidates[a.1];
        let cb = &scans[b.0].candidates[b.1];
        ca.modified
            .partial_cmp(&cb.modified)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| ca.repo.cmp(&cb.repo))
            .then_with(|| ca.commit.cmp(&cb.commit))
    });

    let mut deleted = 0i64;
    let mut selected = 0i64;
    let mut expected_total = 0i64;
    let mut scans = scans;
    for (scan_index, candidate_index) in candidates {
        if Instant::now() >= deadline {
            report.caps.deadline = true;
            break;
        }
        if selected >= policy.max_items_per_pass {
            report.caps.items = true;
            break;
        }
        if free_bytes(home)? >= policy.target_free_gb * GIB {
            break;
        }
        let expected = scans[scan_index].candidates[candidate_index].expected;
        let remaining = policy.max_bytes_per_pass - expected_total;
        if expected > remaining {
            report.skip_hf("byte_cap", 1);
            report.caps.bytes = true;
            continue;
        }
        report.hf.expected_bytes += expected;
        selected += 1;
        if policy.mode != "enforce" {
            expected_total += expected;
            continue;
        }
        if !locks_present {
            report.skip_hf("lock_root_absent", 1);
            break;
        }
        let parts = [
            OsString::from(".cache"),
            OsString::from("huggingface"),
            OsString::from("hub"),
        ];
        // Python `_fixed_root(..., required=True)` and `.stat()` raise
        // straight out of _run_hf here.
        let current_root =
            fixed_root(home, &parts, true)?.expect("required=true never yields None");
        let current_stat = std::fs::metadata(&current_root).map_err(JanitorError::from)?;
        if identity_from_metadata(&current_stat) != identity(&root_info) {
            report.skip_hf("root_changed", 1);
            break;
        }
        let recheck =
            (|scans: &mut Vec<RepoScan>, budget: &mut ScanBudget, report: &mut CleanupReport| {
                recheck_repository_snapshots(
                    root_fd.as_raw_fd(),
                    &root_info,
                    &scans[scan_index],
                    budget,
                    report,
                )?;
                let refs = scans[scan_index].candidates[candidate_index].refs.clone();
                let commit = scans[scan_index].candidates[candidate_index]
                    .commit
                    .to_string_lossy()
                    .into_owned();
                for (path_parts, ident) in &refs {
                    budget.tick(report)?;
                    recheck_ref(root_fd.as_raw_fd(), path_parts, ident, &commit)?;
                }
                let (current_locks, temporary_fds, current_locks_present) = scan_lock_state(
                    root_fd.as_raw_fd(),
                    &root_info,
                    budget,
                    report,
                    false,
                    ".locks",
                    &BTreeSet::new(),
                )?;
                drop(temporary_fds);
                if !current_locks_present || current_locks != lock_state_map {
                    return Err(os_error("cache lock set changed"));
                }
                let known: BTreeSet<Identity> = lock_state_map.values().copied().collect();
                for descriptor in &lock_fds {
                    if !known.contains(&identity(&safefs::fstat(descriptor.as_raw_fd())?)) {
                        return Err(os_error("held cache lock changed"));
                    }
                }
                Ok(())
            })(&mut scans, &mut budget, report);
        if let Err(exc) = recheck {
            report.add_error("huggingface_recheck", &exc);
            break;
        }
        let before = free_bytes(home)?;
        let delete_result =
            (|scans: &mut Vec<RepoScan>, budget: &mut ScanBudget, report: &mut CleanupReport| {
                let barrier_identities = enter_lock_barrier(
                    root_fd.as_raw_fd(),
                    &root_info,
                    &lock_state_map,
                    &lock_fds,
                    budget,
                    report,
                )?;
                let exec_outcome = execute_candidate(
                    root_fd.as_raw_fd(),
                    &scans[scan_index].candidates[candidate_index],
                );
                let leave_outcome =
                    leave_lock_barrier(root_fd.as_raw_fd(), &root_info, barrier_identities);
                // Python `finally:` semantics: a leave error replaces an exec
                // error; an exec error propagates through a clean leave.
                match (exec_outcome, leave_outcome) {
                    (Ok(()), Ok(())) => Ok(()),
                    (Err(exec), Ok(())) => Err(exec),
                    (_, Err(leave)) => Err(leave),
                }
            })(&mut scans, &mut budget, report);
        if let Err(exc) = delete_result {
            report.add_error("huggingface_delete", &exc);
            break;
        }
        scans[scan_index].candidates[candidate_index].deleted = true;
        // Python `root_info = os.fstat(root_fd)`: the barrier dance
        // replaced direct children of the hub root (`.locks` exchange +
        // barrier rmdir), so the cached root identity is stale without
        // this refresh.
        root_info = safefs::fstat(root_fd.as_raw_fd())?;
        let after = free_bytes(home)?;
        let actual = (after - before).max(0);
        deleted += 1;
        expected_total += expected;
        report.hf.deleted_items += 1;
        report.hf.actual_free_delta_bytes += actual;
    }
    Ok((deleted, expected_total))
}

/// Identity built from `std::fs::Metadata` (follows symlinks — used only
/// for the Python `root.stat()` path-based comparison).
fn identity_from_metadata(metadata: &std::fs::Metadata) -> Identity {
    use std::os::unix::fs::MetadataExt;
    Identity {
        dev: metadata.dev(),
        ino: metadata.ino(),
        ifmt: ifmt(metadata.mode()),
        size: metadata.size() as i64,
        mtime_ns: metadata.mtime() * 1_000_000_000 + metadata.mtime_nsec(),
        nlink: metadata.nlink(),
    }
}

#[cfg(test)]
mod tests {
    use super::super::kit::*;
    use super::super::{ensure_state_dir, JanitorError};
    use super::*;
    use serde_json::json;
    use std::os::unix::ffi::OsStringExt;

    fn os(s: &str) -> OsString {
        OsString::from(s)
    }

    // ------------------------------------------------------------------
    // normalize_link (pure lexical guard)
    // ------------------------------------------------------------------

    #[test]
    fn normalize_link_accepts_repo_blobs_only() {
        let repo = vec![os("models--org--name")];
        let link = vec![
            os("models--org--name"),
            os("snapshots"),
            os("c1"),
            os("file"),
        ];
        let ok = normalize_link(&repo, &link, OsStr::new("../../blobs/sha")).unwrap();
        assert_eq!(ok, vec![os("models--org--name"), os("blobs"), os("sha")]);
        // Nested file, deeper relative path.
        let link = vec![
            os("models--org--name"),
            os("snapshots"),
            os("c1"),
            os("sub"),
            os("f"),
        ];
        let ok = normalize_link(&repo, &link, OsStr::new("../../../blobs/sha")).unwrap();
        assert_eq!(ok, vec![os("models--org--name"), os("blobs"), os("sha")]);
        // Redundant ./ segments are fine.
        let ok = normalize_link(&repo, &link, OsStr::new("../../.././blobs/sha")).unwrap();
        assert_eq!(ok, vec![os("models--org--name"), os("blobs"), os("sha")]);
    }

    #[test]
    fn normalize_link_refuses_attacks() {
        let repo = vec![os("models--org--name")];
        let link = vec![
            os("models--org--name"),
            os("snapshots"),
            os("c1"),
            os("file"),
        ];
        // Absolute target (the symlink-to-/etc attack).
        assert!(normalize_link(&repo, &link, OsStr::new("/etc/passwd")).is_err());
        // Escaping above the snapshot parent.
        assert!(normalize_link(&repo, &link, OsStr::new("../../../../../../etc/passwd")).is_err());
        // Staying inside the cache but outside THIS repo's blobs.
        assert!(normalize_link(&repo, &link, OsStr::new("../../refs/main")).is_err());
        assert!(normalize_link(&repo, &link, OsStr::new("../../../other-repo/blobs/sha")).is_err());
        // Empty target.
        assert!(normalize_link(&repo, &link, OsStr::new("")).is_err());
    }

    // ------------------------------------------------------------------
    // unlink_checked (the final identity guard before removal)
    // ------------------------------------------------------------------

    fn open_hub(home: &Path) -> (OwnedFd, FileStat) {
        let root_fd = safefs::open_dir_path(&home.join(".cache/huggingface/hub")).unwrap();
        let root_info = safefs::fstat(root_fd.as_raw_fd()).unwrap();
        (root_fd, root_info)
    }

    fn blob_identity(root_fd: RawFd, repo: &str, blob: &str) -> Identity {
        let parts = vec![os(repo), os("blobs"), os(blob)];
        identity(&leaf_info(root_fd, &parts).unwrap())
    }

    #[test]
    fn unlink_checked_removes_only_the_exact_identity() {
        let th = TempHome::new();
        make_hf_repo(
            &th.home,
            "models--org--name",
            "abc123",
            &[("blobA", b"aaaa")],
        );
        let (root_fd, _root_info) = open_hub(&th.home);
        let parts = vec![os("models--org--name"), os("blobs"), os("blobA")];
        let ident = blob_identity(root_fd.as_raw_fd(), "models--org--name", "blobA");
        unlink_checked(root_fd.as_raw_fd(), &parts, &ident, false).unwrap();
        assert!(!th
            .join(".cache/huggingface/hub/models--org--name/blobs/blobA")
            .exists());
    }

    #[test]
    fn unlink_checked_refuses_drifted_content() {
        let th = TempHome::new();
        let repo = make_hf_repo(
            &th.home,
            "models--org--name",
            "abc123",
            &[("blobA", b"aaaa")],
        );
        let (root_fd, _root_info) = open_hub(&th.home);
        let parts = vec![os("models--org--name"), os("blobs"), os("blobA")];
        let ident = blob_identity(root_fd.as_raw_fd(), "models--org--name", "blobA");
        // Attacker rewrites the blob between scan and deletion.
        std::fs::write(repo.join("blobs/blobA"), b"aaaaaaaaaaaaaaaaaaaa").unwrap();
        assert!(unlink_checked(root_fd.as_raw_fd(), &parts, &ident, false).is_err());
        assert!(repo.join("blobs/blobA").exists());
    }

    #[test]
    fn unlink_checked_refuses_type_swap_to_symlink() {
        let th = TempHome::new();
        let repo = make_hf_repo(
            &th.home,
            "models--org--name",
            "abc123",
            &[("blobA", b"aaaa")],
        );
        let (root_fd, _root_info) = open_hub(&th.home);
        let parts = vec![os("models--org--name"), os("blobs"), os("blobA")];
        let ident = blob_identity(root_fd.as_raw_fd(), "models--org--name", "blobA");
        // Attacker swaps the blob for a symlink to /etc.
        std::fs::remove_file(repo.join("blobs/blobA")).unwrap();
        std::os::unix::fs::symlink("/etc/passwd", repo.join("blobs/blobA")).unwrap();
        assert!(unlink_checked(root_fd.as_raw_fd(), &parts, &ident, false).is_err());
        assert!(repo
            .join("blobs/blobA")
            .symlink_metadata()
            .unwrap()
            .file_type()
            .is_symlink());
        // And a directory where a file was expected is refused too.
        let th2 = TempHome::new();
        let repo2 = make_hf_repo(
            &th2.home,
            "models--org--name",
            "abc123",
            &[("blobA", b"aaaa")],
        );
        let (root_fd2, _) = open_hub(&th2.home);
        let ident2 = blob_identity(root_fd2.as_raw_fd(), "models--org--name", "blobA");
        std::fs::remove_file(repo2.join("blobs/blobA")).unwrap();
        std::fs::create_dir(repo2.join("blobs/blobA")).unwrap();
        assert!(unlink_checked(root_fd2.as_raw_fd(), &parts, &ident2, false).is_err());
        assert!(repo2.join("blobs/blobA").is_dir());
    }

    // ------------------------------------------------------------------
    // scan-time attack trees (nothing may ever be deleted)
    // ------------------------------------------------------------------

    fn enforce_registry() -> serde_json::Value {
        super::super::kit::registry_json(
            "testhost",
            super::super::kit::policy_json(
                "enforce",
                json!({"huggingface_cache": super::super::kit::hf_cleaner()}),
            ),
        )
    }

    fn run_attack(th: &TempHome) -> serde_json::Value {
        super::super::kit::run_pass(th, enforce_registry(), "testhost", 0, false)
    }

    #[test]
    fn cache_root_symlink_is_refused() {
        let th = TempHome::new();
        let real = th.join("real-hub");
        std::fs::create_dir_all(&real).unwrap();
        std::fs::create_dir_all(th.join(".cache/huggingface")).unwrap();
        std::os::unix::fs::symlink(&real, th.join(".cache/huggingface/hub")).unwrap();
        let report = run_attack(&th);
        let hf = &report["cleaners"]["huggingface_cache"];
        assert_eq!(hf["deleted_items"], 0);
        assert!(
            report["errors"]
                .as_array()
                .unwrap()
                .iter()
                .any(|e| e == "huggingface_cache:OSError"),
            "{report}"
        );
        assert!(real.exists());
    }

    #[test]
    fn snapshot_symlink_to_etc_is_refused_and_nothing_deleted() {
        let th = TempHome::new();
        let repo = make_hf_repo(
            &th.home,
            "models--org--name",
            "abc123",
            &[("blobA", b"aaaa")],
        );
        // The classic escape: a snapshot entry pointing at /etc.
        std::fs::remove_file(repo.join("snapshots/abc123/file1.txt")).unwrap();
        std::os::unix::fs::symlink("/etc/passwd", repo.join("snapshots/abc123/file1.txt")).unwrap();
        backdate_tree(&repo, 2 * 3600);
        let report = run_attack(&th);
        let hf = &report["cleaners"]["huggingface_cache"];
        assert_eq!(hf["deleted_items"], 0, "{report}");
        assert!(
            report["errors"]
                .as_array()
                .unwrap()
                .iter()
                .any(|e| e == "huggingface_cache:OSError"),
            "{report}"
        );
        assert!(repo.join("snapshots/abc123").exists());
        assert!(repo.join("blobs/blobA").exists());
        assert!(Path::new("/etc/passwd").exists());
    }

    #[test]
    fn snapshot_symlink_escaping_repo_is_refused() {
        let th = TempHome::new();
        let repo = make_hf_repo(
            &th.home,
            "models--org--name",
            "abc123",
            &[("blobA", b"aaaa")],
        );
        std::fs::remove_file(repo.join("snapshots/abc123/file1.txt")).unwrap();
        // Stays in the cache but leaves the repository's blobs.
        std::os::unix::fs::symlink("../../refs/main", repo.join("snapshots/abc123/file1.txt"))
            .unwrap();
        backdate_tree(&repo, 2 * 3600);
        let report = run_attack(&th);
        assert_eq!(report["cleaners"]["huggingface_cache"]["deleted_items"], 0);
        assert!(repo.join("snapshots/abc123").exists());
    }

    #[test]
    fn snapshot_referencing_unknown_blob_is_refused() {
        let th = TempHome::new();
        let repo = make_hf_repo(
            &th.home,
            "models--org--name",
            "abc123",
            &[("blobA", b"aaaa")],
        );
        std::fs::remove_file(repo.join("snapshots/abc123/file1.txt")).unwrap();
        std::os::unix::fs::symlink("../../blobs/ghost", repo.join("snapshots/abc123/file1.txt"))
            .unwrap();
        backdate_tree(&repo, 2 * 3600);
        let report = run_attack(&th);
        assert_eq!(report["cleaners"]["huggingface_cache"]["deleted_items"], 0);
        assert!(repo.join("snapshots/abc123").exists());
    }

    #[test]
    fn symlinked_snapshot_dir_and_blob_are_refused() {
        let th = TempHome::new();
        let repo = make_hf_repo(
            &th.home,
            "models--org--name",
            "abc123",
            &[("blobA", b"aaaa")],
        );
        // Replace the snapshot dir with a symlink.
        let target = th.join("evil-snapshot");
        std::fs::create_dir(&target).unwrap();
        std::fs::remove_dir_all(repo.join("snapshots/abc123")).unwrap();
        std::os::unix::fs::symlink(&target, repo.join("snapshots/abc123")).unwrap();
        let report = run_attack(&th);
        assert_eq!(report["cleaners"]["huggingface_cache"]["deleted_items"], 0);
        assert!(target.exists());

        // A symlinked blob is refused outright.
        let th2 = TempHome::new();
        let repo2 = make_hf_repo(
            &th2.home,
            "models--org--name",
            "abc123",
            &[("blobA", b"aaaa")],
        );
        let outside = th2.join("outside-blob");
        std::fs::write(&outside, b"outside").unwrap();
        std::fs::remove_file(repo2.join("blobs/blobA")).unwrap();
        std::os::unix::fs::symlink(&outside, repo2.join("blobs/blobA")).unwrap();
        backdate_tree(&repo2, 2 * 3600);
        let report = run_attack(&th2);
        assert_eq!(
            report["cleaners"]["huggingface_cache"]["deleted_items"], 0,
            "{report}"
        );
        assert!(outside.exists());
    }

    #[test]
    fn symlinked_lock_file_is_refused() {
        let th = TempHome::new();
        let repo = make_hf_repo(
            &th.home,
            "models--org--name",
            "abc123",
            &[("blobA", b"aaaa")],
        );
        let lock_path = th.join(".cache/huggingface/hub/.locks/models--org--name/abc123.lock");
        std::fs::remove_file(&lock_path).unwrap();
        let outside = th.join("held-elsewhere");
        std::fs::write(&outside, b"").unwrap();
        std::os::unix::fs::symlink(&outside, &lock_path).unwrap();
        backdate_tree(&repo, 2 * 3600);
        let report = run_attack(&th);
        assert_eq!(
            report["cleaners"]["huggingface_cache"]["deleted_items"], 0,
            "{report}"
        );
        assert!(repo.join("snapshots/abc123").exists());
    }

    #[test]
    fn hardlinked_blob_is_not_deleted_and_reports_uncertain() {
        let th = TempHome::new();
        let repo = make_hf_repo(
            &th.home,
            "models--org--name",
            "abc123",
            &[("blobA", b"aaaa")],
        );
        // A second hard link to the same blob inode (nlink = 2): the
        // janitor cannot prove exclusivity and must keep the blob.
        let second_link = th.join(".cache/huggingface/hub/models--org--name/blobs/blobA-copy");
        std::fs::hard_link(repo.join("blobs/blobA"), &second_link).unwrap();
        backdate_tree(&repo, 2 * 3600);
        let report = run_attack(&th);
        let hf = &report["cleaners"]["huggingface_cache"];
        assert_eq!(hf["skipped"]["blob_link_count_uncertain"], 1, "{report}");
        assert_eq!(hf["deleted_items"], 1);
        // Snapshot deleted, but the blob survives (both names).
        assert!(!repo.join("snapshots/abc123").exists());
        assert!(second_link.exists());
    }

    #[test]
    fn shared_blob_survives_deleting_one_of_two_revisions() {
        let th = TempHome::new();
        let repo = make_hf_repo(
            &th.home,
            "models--org--name",
            "aaaaaa",
            &[("shared", b"data")],
        );
        // Second revision referencing the same blob.
        std::fs::create_dir_all(repo.join("snapshots/bbbbbb")).unwrap();
        std::os::unix::fs::symlink("../../blobs/shared", repo.join("snapshots/bbbbbb/f")).unwrap();
        std::fs::write(
            th.join(".cache/huggingface/hub/.locks/models--org--name/bbbbbb.lock"),
            b"",
        )
        .unwrap();
        backdate_tree(&repo, 3 * 3600);
        backdate_tree(&repo.join("snapshots/bbbbbb"), 2 * 3600);
        let report = run_attack(&th);
        let hf = &report["cleaners"]["huggingface_cache"];
        assert_eq!(hf["deleted_items"], 2, "{report}");
        assert!(repo.join("blobs/shared").exists());
    }

    #[test]
    fn incomplete_download_marks_repository_incomplete() {
        let th = TempHome::new();
        let repo = make_hf_repo(
            &th.home,
            "models--org--name",
            "abc123",
            &[("blobA", b"aaaa")],
        );
        std::fs::write(repo.join("blobs/blobB.incomplete"), b"partial").unwrap();
        backdate_tree(&repo, 2 * 3600);
        let report = run_attack(&th);
        let hf = &report["cleaners"]["huggingface_cache"];
        assert_eq!(hf["skipped"]["incomplete_repository"], 1, "{report}");
        assert_eq!(hf["deleted_items"], 0);
        assert!(repo.join("snapshots/abc123").exists());
    }

    #[test]
    fn tampered_blob_is_caught_by_recheck_before_deletion() {
        let th = TempHome::new();
        let repo = make_hf_repo(
            &th.home,
            "models--org--name",
            "abc123",
            &[("blobA", b"aaaa")],
        );
        backdate_tree(&repo, 2 * 3600);
        // Directly exercise the recheck: scan, tamper, recheck must fail.
        let (root_fd, root_info) = open_hub(&th.home);
        let mut report = super::super::CleanupReport::base(0, "testhost");
        let mut budget = super::super::ScanBudget::new(
            10000,
            std::time::Instant::now() + std::time::Duration::from_secs(30),
        );
        let scans = scan_cache(root_fd.as_raw_fd(), &root_info, &mut budget, &mut report).unwrap();
        std::fs::write(repo.join("blobs/blobA"), b"tampered-with-different-size").unwrap();
        let err = recheck_repository_snapshots(
            root_fd.as_raw_fd(),
            &root_info,
            &scans[0],
            &mut budget,
            &mut report,
        )
        .unwrap_err();
        assert_eq!(err.code, "OSError");
    }

    #[test]
    fn added_snapshot_between_scan_and_recheck_aborts() {
        let th = TempHome::new();
        let repo = make_hf_repo(
            &th.home,
            "models--org--name",
            "abc123",
            &[("blobA", b"aaaa")],
        );
        backdate_tree(&repo, 2 * 3600);
        let (root_fd, root_info) = open_hub(&th.home);
        let mut report = super::super::CleanupReport::base(0, "testhost");
        let mut budget = super::super::ScanBudget::new(
            10000,
            std::time::Instant::now() + std::time::Duration::from_secs(30),
        );
        let scans = scan_cache(root_fd.as_raw_fd(), &root_info, &mut budget, &mut report).unwrap();
        // A new revision appears after the scan: the snapshot set changed.
        std::fs::create_dir_all(repo.join("snapshots/ffffff")).unwrap();
        std::fs::write(repo.join("blobs/blobF"), b"f").unwrap();
        std::os::unix::fs::symlink("../../blobs/blobF", repo.join("snapshots/ffffff/f")).unwrap();
        assert!(recheck_repository_snapshots(
            root_fd.as_raw_fd(),
            &root_info,
            &scans[0],
            &mut budget,
            &mut report,
        )
        .is_err());
    }

    // ------------------------------------------------------------------
    // barrier dance + crash recovery
    // ------------------------------------------------------------------

    #[test]
    fn barrier_roundtrip_preserves_lock_namespace() {
        let th = TempHome::new();
        make_hf_repo(
            &th.home,
            "models--org--name",
            "abc123",
            &[("blobA", b"aaaa")],
        );
        let (root_fd, root_info) = open_hub(&th.home);
        let mut report = super::super::CleanupReport::base(0, "testhost");
        let mut budget = super::super::ScanBudget::new(
            10000,
            std::time::Instant::now() + std::time::Duration::from_secs(30),
        );
        let (state, held, present) = scan_lock_state(
            root_fd.as_raw_fd(),
            &root_info,
            &mut budget,
            &mut report,
            true,
            ".locks",
            &BTreeSet::new(),
        )
        .unwrap();
        assert!(present);
        assert_eq!(held.len(), 1);
        let identities = enter_lock_barrier(
            root_fd.as_raw_fd(),
            &root_info,
            &state,
            &held,
            &mut budget,
            &mut report,
        )
        .unwrap();
        // Mid-barrier: `.locks` is the marked private copy, the original
        // namespace sits at the barrier name.
        assert!(th
            .join(".cache/huggingface/hub/.locks/.wisent-compute-barrier")
            .exists());
        assert!(th
            .join(
                ".cache/huggingface/hub/.wisent-compute-lock-barrier/models--org--name/abc123.lock"
            )
            .exists());
        leave_lock_barrier(root_fd.as_raw_fd(), &root_info, identities).unwrap();
        assert!(th
            .join(".cache/huggingface/hub/.locks/models--org--name/abc123.lock")
            .exists());
        assert!(!th
            .join(".cache/huggingface/hub/.locks/.wisent-compute-barrier")
            .exists());
        assert!(!th
            .join(".cache/huggingface/hub/.wisent-compute-lock-barrier")
            .exists());
    }

    #[test]
    fn recovery_discards_interrupted_barrier() {
        let th = TempHome::new();
        make_hf_repo(
            &th.home,
            "models--org--name",
            "abc123",
            &[("blobA", b"aaaa")],
        );
        let hub = th.join(".cache/huggingface/hub");
        let (root_fd, root_info) = open_hub(&th.home);
        // Simulate a crash AFTER prepare but BEFORE the exchange: private
        // copy marked, canonical untouched.
        let barrier = hub.join(".wisent-compute-lock-barrier");
        std::fs::create_dir(&barrier).unwrap();
        std::fs::write(barrier.join(".wisent-compute-barrier"), b"").unwrap();
        recover_lock_barrier(root_fd.as_raw_fd(), &root_info).unwrap();
        assert!(!barrier.exists());
        assert!(hub.join(".locks/models--org--name/abc123.lock").exists());

        // Simulate a crash AFTER the exchange: the canonical `.locks`
        // holds the marked private copy, while the original (unmarked)
        // namespace sits at the barrier name.
        let barrier = hub.join(".wisent-compute-lock-barrier");
        std::fs::create_dir_all(barrier.join("models--org--name")).unwrap();
        std::fs::write(barrier.join("models--org--name/abc123.lock"), b"").unwrap();
        std::fs::remove_file(hub.join(".locks/models--org--name/abc123.lock")).unwrap();
        std::fs::write(hub.join(".locks/.wisent-compute-barrier"), b"").unwrap();
        recover_lock_barrier(root_fd.as_raw_fd(), &root_info).unwrap();
        assert!(!barrier.exists());
        assert!(!hub.join(".locks/.wisent-compute-barrier").exists());
        assert!(hub.join(".locks/models--org--name/abc123.lock").exists());

        // Both marked: ambiguous residue refuses to run.
        let barrier = hub.join(".wisent-compute-lock-barrier");
        std::fs::create_dir(&barrier).unwrap();
        std::fs::write(barrier.join(".wisent-compute-barrier"), b"").unwrap();
        std::fs::write(hub.join(".locks/.wisent-compute-barrier"), b"").unwrap();
        let err = recover_lock_barrier(root_fd.as_raw_fd(), &root_info).unwrap_err();
        assert_eq!(err.code, "OSError");
    }

    #[test]
    fn scan_rejects_unknown_layouts() {
        let th = TempHome::new();
        make_hf_repo(
            &th.home,
            "models--org--name",
            "abc123",
            &[("blobA", b"aaaa")],
        );
        // Stray file at the cache root.
        std::fs::write(th.join(".cache/huggingface/hub/random-file"), b"x").unwrap();
        let report = run_attack(&th);
        assert_eq!(report["cleaners"]["huggingface_cache"]["deleted_items"], 0);
        assert!(th.join(".cache/huggingface/hub/random-file").exists());

        // Direct layout mixed with repository layout.
        let th2 = TempHome::new();
        make_hf_repo(
            &th2.home,
            "models--org--name",
            "abc123",
            &[("blobA", b"aaaa")],
        );
        std::fs::create_dir_all(th2.join(".cache/huggingface/hub/blobs")).unwrap();
        let report = run_attack(&th2);
        assert_eq!(
            report["cleaners"]["huggingface_cache"]["deleted_items"], 0,
            "{report}"
        );
    }

    #[test]
    fn invalid_ref_contents_abort_the_repo() {
        let th = TempHome::new();
        let repo = make_hf_repo(
            &th.home,
            "models--org--name",
            "abc123",
            &[("blobA", b"aaaa")],
        );
        std::fs::write(repo.join("refs/main"), "not-a-hex-commit!!").unwrap();
        backdate_tree(&repo, 2 * 3600);
        let report = run_attack(&th);
        assert_eq!(
            report["cleaners"]["huggingface_cache"]["deleted_items"], 0,
            "{report}"
        );
        assert!(repo.join("snapshots/abc123").exists());
    }

    #[test]
    fn empty_snapshot_is_refused() {
        let th = TempHome::new();
        let repo = make_hf_repo(
            &th.home,
            "models--org--name",
            "abc123",
            &[("blobA", b"aaaa")],
        );
        std::fs::create_dir_all(repo.join("snapshots/empty00")).unwrap();
        backdate_tree(&repo, 2 * 3600);
        let report = run_attack(&th);
        // The empty snapshot poisons the whole scan (fail closed).
        assert_eq!(
            report["cleaners"]["huggingface_cache"]["deleted_items"], 0,
            "{report}"
        );
        assert!(repo.join("snapshots/abc123").exists());
    }

    #[test]
    fn scan_budget_cap_stops_and_reports() {
        let th = TempHome::new();
        let repo = make_hf_repo(
            &th.home,
            "models--org--name",
            "abc123",
            &[("blobA", b"aaaa")],
        );
        backdate_tree(&repo, 2 * 3600);
        let mut policy = super::super::kit::policy_json(
            "enforce",
            json!({"huggingface_cache": super::super::kit::hf_cleaner()}),
        );
        policy["max_scan_items"] = json!(1);
        policy["max_items_per_pass"] = json!(1);
        let registry = registry_json("testhost", policy);
        let report = super::super::kit::run_pass(&th, registry, "testhost", 0, false);
        let hf = &report["cleaners"]["huggingface_cache"];
        assert_eq!(hf["skipped"]["scan_cap"], 1, "{report}");
        assert_eq!(report["caps"]["scan"], true);
        assert_eq!(hf["deleted_items"], 0);
        assert!(repo.join("snapshots/abc123").exists());
    }

    #[test]
    fn identity_tuple_and_type_checks() {
        let th = TempHome::new();
        make_hf_repo(
            &th.home,
            "models--org--name",
            "abc123",
            &[("blobA", b"aaaa")],
        );
        let (root_fd, _root_info) = open_hub(&th.home);
        let parts: Vec<OsString> = vec![OsString::from_vec(b"models--org--name".to_vec())];
        let fd = safefs::open_path(root_fd.as_raw_fd(), &parts).unwrap();
        let info = safefs::fstat(fd.as_raw_fd()).unwrap();
        assert_eq!(identity(&info).ifmt, IFDIR);
        let _ = JanitorError::os("x"); // error type sanity
    }

    #[test]
    fn state_dir_name_is_stable() {
        // The reporting paths must match the Python byte-for-byte.
        let th = TempHome::new();
        let dir = ensure_state_dir(&th.home).unwrap();
        assert!(dir.ends_with(".cache/wisent-compute"));
        assert!(th.join(".cache/wisent-compute").is_dir());
    }
}
