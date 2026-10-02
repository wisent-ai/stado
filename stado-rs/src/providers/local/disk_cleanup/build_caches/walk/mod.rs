//! The inventory walk: the bounded, level-order traversal that finds the
//! directories their own build tool declared regenerable.
//!
//! Layout: [`tag`] reads `CACHEDIR.TAG` and reports what it authorizes,
//! [`sizing`] totals the bytes one tagged tree occupies, [`judge`] decides
//! one tagged tree and — in `enforce` — reclaims it, and [`examine`] is one
//! frontier directory's worth of children. This module owns the state all
//! four share, the age gate, the scan budget and the level-order loop.

mod examine;
mod judge;
mod sizing;
mod tag;

use std::collections::VecDeque;
use std::os::fd::{AsRawFd, RawFd};
use std::path::{Path, PathBuf};

use nix::libc::dev_t;
use nix::sys::stat::FileStat;

use crate::providers::local::disk_cleanup::build_caches::cursor::CursorPath;
use crate::providers::local::disk_cleanup::build_caches::walk::tag::Tag;
use crate::providers::local::disk_cleanup::build_caches::MAX_DEPTH;
use crate::providers::local::disk_cleanup::consent::{self, Gated};
use crate::providers::local::disk_cleanup::{euid, safefs, CleanupReport, JanitorError};
use crate::targets::{DiskCleanerPolicy, DiskCleanupPolicy};

/// Whether the walk may continue at all, or has spent a pass-wide budget.
pub(super) enum Progress {
    Continue,
    /// Scan cap or the free-space target: the pass is done and
    /// every remaining directory is left unexamined rather than half-judged.
    Halt,
}

/// The scan's shared state: the budgets it spends and the root it may not
/// leave.
pub(super) struct Walk<'a> {
    pub(super) home: &'a Path,
    pub(super) policy: &'a DiskCleanupPolicy,
    pub(super) configured: &'a DiskCleanerPolicy,
    /// Epoch seconds the pass started (Python-style `time.time()`).
    pub(super) now: f64,
    /// Directories left in this pass's share of `max_scan_items`.
    pub(super) remaining_scan: i64,
    /// `st_dev` of the scan root. A mount point inside the tree is refused
    /// rather than descended: an external disk or a network share mounted
    /// under a tagged directory is not what the build tool declared
    /// regenerable.
    pub(super) root_dev: dev_t,
    pub(super) reserved: Vec<PathBuf>,
    /// Roots the walk must not even look inside, because looking is what
    /// costs: a macOS privacy prompt, or a cloud download.
    pub(super) privacy: Vec<PathBuf>,
    /// Folders with an asynchronous macOS consent read. A pending answer is
    /// reported without holding this pass open.
    pub(super) gated: Vec<PathBuf>,
    /// Bytes this pass expects to have freed, against `max_bytes_per_pass`.
    pub(super) deleted_bytes: i64,
    /// Directories discovered but not yet fully examined, in breadth-first
    /// order. Unlike the former positional cursor, this queue is durable:
    /// resuming never has to reconstruct old levels before doing new work.
    pub(super) frontier: VecDeque<CursorPath>,
    /// First unexamined child of the directory at the front of `frontier`.
    /// `None` means that parent has not been started.
    pub(super) next_child: Option<PathBuf>,
}

impl<'a> Walk<'a> {
    /// Age gate. The directory's own mtime is what
    /// `find "$dir" -maxdepth 0 -mtime +N` tests in the remote script.
    fn old_enough(&self, info: &FileStat) -> bool {
        (info.st_mtime as f64) <= self.now - self.configured.min_age_seconds as f64
    }

    /// Charge one directory to the scan budget, or report which pass-wide
    /// limit stopped the walk.
    fn charge(&mut self, report: &mut CleanupReport) -> Progress {
        if self.remaining_scan <= 0 {
            report.caps.scan = true;
            report.skip_builds("scan_cap", 1);
            return Progress::Halt;
        }
        self.remaining_scan -= 1;
        report.builds.scanned_items += 1;
        Progress::Continue
    }

    /// Examine the tree one level at a time, stopping at every directory its
    /// own build tool tagged as regenerable.
    ///
    /// LEVEL ORDER, and that is the whole of why this cleaner now finds
    /// anything. A build tool writes its cache at the TOP of the tree it
    /// generates — that is where the standard puts `CACHEDIR.TAG` — so every
    /// candidate is shallow and everything deep is some tree's contents. A
    /// depth-first walk spends its budget the other way round: crossing a
    /// workspace root can take hundreds of thousands of directories against a
    /// `max_scan_items` of 100,000, with the walk still inside the first
    /// repository's `node_modules`, having examined none of the tagged caches
    /// the tree holds. Visiting by level reaches most of them inside one
    /// pass's budget, on the same tree with the same cap.
    ///
    /// The order changes only WHICH directories a bounded pass gets to look
    /// at. Every deletion criterion — the tag, the age, the reserved roots,
    /// the ownership and device checks — is applied exactly as before.
    pub(super) fn walk_levels(
        &mut self,
        root_fd: RawFd,
        root: &Path,
        report: &mut CleanupReport,
    ) -> Result<Progress, JanitorError> {
        while let Some(parent) = self.frontier.pop_front().map(PathBuf::from) {
            let depth = parent.components().count();
            if depth >= MAX_DEPTH {
                self.next_child = None;
                report.skip_builds("depth_cap", 1);
                continue;
            }
            // A durable queue is only a location hint. Reopen and revalidate
            // every component against today's tree and policy before using it.
            let parent_fd = (|| -> Result<Option<std::os::fd::OwnedFd>, JanitorError> {
                let mut descriptor = safefs::dup_fd(root_fd)?;
                let mut absolute = root.to_path_buf();
                for part in parent.components() {
                    absolute.push(part);
                    let child = match consent::open_dir_at(
                        &self.gated,
                        descriptor.as_raw_fd(),
                        part.as_os_str(),
                        &absolute,
                    )? {
                        Gated::Opened(child) => child,
                        Gated::Pending => return Ok(None),
                    };
                    let info = safefs::fstat(child.as_raw_fd())?;
                    if info.st_uid != euid()
                        || info.st_dev != self.root_dev
                        || self.reserved.iter().any(|item| absolute.starts_with(item))
                    {
                        return Err(JanitorError::os("queued directory is no longer safe"));
                    }
                    let guards_reserved =
                        self.reserved.iter().any(|item| item.starts_with(&absolute));
                    if !guards_reserved && matches!(self.read_tag(child.as_raw_fd())?, Tag::Signed)
                    {
                        return Err(JanitorError::os("queued ancestor is now a tagged cache"));
                    }
                    descriptor = child;
                }
                Ok(Some(descriptor))
            })();
            let parent_fd = match parent_fd {
                Ok(Some(descriptor)) => descriptor,
                Ok(None) => {
                    // The person at the keyboard has the question; the pass
                    // keeps its place and ends here, so the lock it holds is
                    // not what waits for the answer.
                    self.frontier.push_front(parent.into());
                    report.skip_builds("consent_pending", 1);
                    return Ok(Progress::Halt);
                }
                Err(_) => {
                    self.next_child = None;
                    report.skip_builds("entry_replaced", 1);
                    continue;
                }
            };
            match self.examine(parent_fd.as_raw_fd(), root, &parent, report) {
                Ok(Progress::Continue) => self.next_child = None,
                result => {
                    self.frontier.push_front(parent.into());
                    return result;
                }
            }
        }
        Ok(Progress::Continue)
    }
}
