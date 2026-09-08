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
//! directories safely from the target's declared cleaner. This module is the
//! same judgement inside the automatic pass.
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
//! Layout: [`cursor`] is the durable checkpoint one bounded pass hands to
//! the next, [`reserved`] the roots this cleaner may never reclaim whatever
//! their tag says, [`walk`] the level-order inventory and the verdict on
//! each tagged directory it reaches, and [`remove`] the deletion itself.
//! This module owns the listing and identity primitives all of them share,
//! the depth limit that is also the descriptor budget, and the cleaner entry
//! point [`scan_build_caches`].

mod cursor;
mod remove;
mod reserved;
mod walk;

use std::collections::BTreeSet;
use std::ffi::OsString;
use std::os::fd::{AsRawFd, RawFd};
use std::path::{Path, PathBuf};
use std::time::Instant;

use nix::sys::stat::FileStat;

use super::safefs;
use super::{euid, ifmt, CleanupReport, JanitorError};
use crate::targets::DiskCleanupPolicy;

use cursor::CursorPath;
use reserved::reserved_roots;
use walk::Walk;

pub(super) use cursor::BuildCachesCursor;

/// One open directory per level is held while the walk is inside it, so the
/// depth limit is also the fd budget. 64 is far below any macOS descriptor
/// limit and far above any real build tree: a `target/` is five or six deep,
/// and the deepest `node_modules` chains npm still produces are around
/// thirty.
const MAX_DEPTH: usize = 64;

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
/// a deterministic order, so two passes over an unchanged tree spend the
/// scan budget on the same directories.
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

/// Scan the build-cache root and evict every directory its own build tool
/// tagged as regenerable.
///
/// `remaining_scan` is this cleaner's share of `max_scan_items` left by the
/// cleaners that ran before it, and `deadline` is the pass deadline the HF
/// scan also honours: unlike the other two roots, this one can be the whole
/// of `$HOME`, where the walk — not the deletion — is the expensive half.
///
/// The durable frontier contains the unvisited directories, not merely the
/// position of the last visit. Older positional cursors restart once to build
/// this queue; subsequent passes open the next parent directly. Replaying all
/// prior levels used to consume the entire deadline with zero newly scanned
/// directories on every pass.
///
/// Neither the order nor the cursor changes WHICH directories may be
/// deleted. Every criterion — the tag, the age, the reserved roots, the
/// ownership and device checks — is applied exactly as it would be on a walk
/// that started at the root and went straight down.
pub(super) fn scan_build_caches(
    home: &Path,
    policy: &DiskCleanupPolicy,
    now: f64,
    remaining_scan: i64,
    deadline: Instant,
    cursor: Option<BuildCachesCursor>,
    report: &mut CleanupReport,
) {
    // A pass that declines to walk must preserve its existing checkpoint.
    report.builds_cursor = cursor;
    let Some(configured) = policy.cleaners.get("build_caches") else {
        return;
    };
    if remaining_scan <= 0 {
        return;
    }
    let body = |report: &mut CleanupReport| -> Result<(), JanitorError> {
        let root = match &configured.root {
            Some(configured_root) => {
                let expanded = crate::config_file::expand_tilde(configured_root);
                if !expanded.is_dir() {
                    report.skip_builds("root_absent", 1);
                    return Ok(());
                }
                std::fs::canonicalize(&expanded)?
            }
            None => home.to_path_buf(),
        };
        let root_fd = safefs::open_dir_path(&root)?;
        let root_info = safefs::fstat(root_fd.as_raw_fd())?;
        if root_info.st_uid != euid() {
            return Err(JanitorError::os("build cache root ownership mismatch"));
        }
        let cursor = report
            .builds_cursor
            .take()
            .filter(|cursor| cursor.valid_for(&root))
            .unwrap_or_else(|| BuildCachesCursor::fresh(root.clone()));
        let mut walk = Walk {
            home,
            policy,
            configured,
            now,
            deadline,
            remaining_scan,
            root_dev: root_info.st_dev,
            reserved: reserved_roots(home, policy),
            deleted_bytes: 0,
            frontier: cursor.frontier,
            next_child: cursor.next_child.map(PathBuf::from),
        };
        // The root itself is never a candidate. Its queued children are
        // revalidated through directory descriptors before they are used.
        let result = walk.walk_levels(root_fd.as_raw_fd(), &root, report);
        report.builds_cursor = if walk.frontier.is_empty() {
            None
        } else {
            Some(BuildCachesCursor {
                version: 1,
                root: root.into(),
                frontier: walk.frontier,
                next_child: walk.next_child.map(CursorPath::from),
            })
        };
        report.builds_resume_from = report
            .builds_cursor
            .as_ref()
            .and_then(BuildCachesCursor::resume_label);
        result.map(|_| ())
    };
    if let Err(exc) = body(report) {
        report.add_error("build_caches", &exc);
    }
}
