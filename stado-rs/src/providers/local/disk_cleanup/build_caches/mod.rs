//! Build-cache cleanup: eviction of directories their own build tool has
//! declared regenerable, by the Cache Directory Tagging Standard.
//!
//! NO Python original. This cleaner exists because the janitor's two
//! consumers, `huggingface_cache` and `weles_recordings`, together describe
//! almost nothing of what actually fills a developer host. The operator's
//! laptop reached 8.8 GB free of 1.8 TB while carrying roughly 450 GB of
//! build and scratch trees — 620 GB of them across the checked-out
//! repositories at the worst point — and `disk-cleanup` had nothing to say
//! about any of it: the host's policy was `mode: "off"`, and even armed it
//! would have reported a healthy no-op, because not one of those directories
//! belongs to a cleaner it knows. A janitor that reports "nothing to do" on a
//! disk that is 99.5% full is worse than no janitor, because the number is
//! believed.
//!
//! `stado space report` ([`crate::deploy::host_build_caches`]) recognises such
//! directories safely under the same home root. This module is the same
//! judgement inside the automatic pass.
//!
//! The safety criterion is that module's, unchanged and imported rather than
//! copied: a directory may be deleted if and only if it contains a
//! `CACHEDIR.TAG` whose first line is
//! [`host_build_caches::CACHEDIR_SIGNATURE`]. Cargo — and cmake, and many
//! others — write that file precisely so a cleaner may remove the directory
//! without asking. No directory-name matching, no extension lists: the tool
//! that produced the bytes is the only party that gets to say they are
//! reproducible.
//!
//! Unlike [`super::weles`], which is a path-based port, every operation here
//! is dir-fd-relative through [`super::safefs`], as in [`super::hf`]. The
//! scan root is the whole of `$HOME` by default, which is the one root in the
//! janitor no one can enumerate in advance; with plain paths, a symlink
//! swapped in mid-walk anywhere in that tree would be a recursive delete of
//! whatever it pointed at.
//!
//! Layout: [`reserved`] holds the roots this cleaner may never reclaim
//! whatever their tag says, [`walk`] the level-order inventory and the
//! verdict on each tagged directory it reaches, [`remove`] the deletion
//! itself, and [`placement`] where Cargo builds when a folder holding
//! checkouts refuses this process. This module owns the listing and identity
//! primitives all of them share, the depth limit that is also the descriptor
//! budget, and the cleaner entry point [`scan_build_caches`].

mod placement;
mod remove;
mod reserved;
mod walk;

use std::collections::{BTreeSet, VecDeque};
use std::ffi::OsString;
use std::os::fd::{AsRawFd, RawFd};
use std::path::{Path, PathBuf};

use nix::sys::stat::FileStat;

use super::{consent, euid, ifmt, safefs, CleanupReport, JanitorError};

use reserved::{privacy_protected_roots, reserved_roots};
use walk::Walk;

pub use reserved::privacy_protected_parts;

/// The one identity comparison this module needs: a directory opened with
/// `O_NOFOLLOW` must be the exact object the preceding `fstatat` described,
/// or something replaced it between the two calls.
#[allow(clippy::unnecessary_cast)] // libc field widths differ per OS; the cast is required on macOS
fn same_object(first: &FileStat, second: &FileStat) -> bool {
    first.st_dev == second.st_dev
        && first.st_ino == second.st_ino
        && ifmt(first.st_mode as u32) == ifmt(second.st_mode as u32)
}

/// One `os.scandir` worth of names, with the two self-references dropped and
/// a deterministic order, so two passes over an unchanged tree visit the
/// same directories in the same order.
fn entry_names(dir_fd: RawFd) -> Result<BTreeSet<OsString>, JanitorError> {
    let mut names = BTreeSet::new();
    for name in safefs::DirEntries::open(dir_fd)? {
        let name = name?;
        if name == "." || name == ".." {
            continue;
        }
        names.insert(name);
    }
    Ok(names)
}

/// Scan the whole home and evict every directory its own build tool tagged
/// as regenerable.
///
/// Every criterion — the tag, the reserved roots, the ownership and device
/// checks — is applied to each directory the level-order walk reaches.
pub(super) fn scan_build_caches(home: &Path, enforcing: bool, report: &mut CleanupReport) {
    placement::place_cargo_builds(home, report);
    let body = |report: &mut CleanupReport| -> Result<(), JanitorError> {
        let root = home.to_path_buf();
        let gated = consent::gated_folders(home);
        let root_fd = match consent::open_dir_path(&gated, &root)
            .map_err(|error| refused_root(&root, &error))?
        {
            consent::Gated::Opened(root_fd) => root_fd,
            consent::Gated::Pending => {
                report.skip_builds("consent_pending", 1);
                return Ok(());
            }
        };
        let root_info = safefs::fstat(root_fd.as_raw_fd())?;
        if root_info.st_uid != euid() {
            return Err(JanitorError::os("build cache root ownership mismatch"));
        }
        let mut walk = Walk {
            home,
            enforcing,
            root_dev: root_info.st_dev,
            reserved: reserved_roots(home),
            privacy: privacy_protected_roots(home),
            gated,
            frontier: VecDeque::from([PathBuf::new()]),
        };
        // The root itself is never a candidate. Its queued children are
        // revalidated through directory descriptors before they are used.
        walk.walk_levels(root_fd.as_raw_fd(), &root, report)
    };
    if let Err(exc) = body(report) {
        report.add_error("build_caches", &exc);
    }
}

/// The home could not be opened, said so that an operator can act.
///
/// A bare `PermissionError (Operation not permitted (os error 1))` with
/// `scanned_items: 0` beside it names neither the root nor the reason. On
/// macOS that errno is the one the
/// operating system returns for a folder behind its own privacy consent —
/// `~/Documents` among them — which is a different repair from a mode bit:
/// the process needs Full Disk Access, or the declaration needs a root the
/// fleet's own agent may read.
fn refused_root(root: &Path, error: &std::io::Error) -> JanitorError {
    let privacy = cfg!(target_os = "macos") && error.raw_os_error() == Some(1);
    let remedy = if privacy {
        "the operating system's privacy protection refuses it to this process: grant Full Disk \
         Access to the agent that runs the janitor"
    } else {
        "the account running the janitor must be able to read its own home"
    };
    JanitorError::os(&format!(
        "the build cache root {} could not be opened ({error}); {remedy}",
        root.display()
    ))
}
