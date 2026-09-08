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
//!
//! Layout: [`inventory`] is that scan — what the cache root holds and which
//! revisions it offers; [`reclaim`] is the removal — the lock barrier, the
//! pre-deletion rechecks and the identity-checked unlink; [`run`] is the
//! cleaner entry point that decides which candidates to evict, spends the
//! pass budget and writes the report. This module owns the identity model
//! every one of them shares.

mod inventory;
mod reclaim;
mod run;

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::fs::File;

use nix::sys::stat::FileStat;

use super::{euid, ifmt, JanitorError};

pub use run::run_hf;

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
