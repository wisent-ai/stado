//! The inventory walk: the level-order traversal that finds the directories
//! their own build tool declared regenerable.
//!
//! Layout: [`tag`] reads `CACHEDIR.TAG` and reports what it authorizes,
//! [`sizing`] totals the bytes one tagged tree occupies, [`judge`] decides
//! one tagged tree and — when enforcing — reclaims it, and [`examine`] is one
//! frontier directory's worth of children. This module owns the state all
//! four share and the level-order loop.

mod examine;
mod judge;
mod sizing;
mod tag;

use std::collections::VecDeque;
use std::os::fd::{AsRawFd, RawFd};
use std::path::{Path, PathBuf};

use nix::libc::dev_t;

use crate::providers::local::disk_cleanup::build_caches::walk::tag::Tag;
use crate::providers::local::disk_cleanup::consent::{self, Gated};
use crate::providers::local::disk_cleanup::{euid, safefs, CleanupReport, JanitorError};

/// The scan's shared state: the root it may not leave and what it refuses.
pub(super) struct Walk<'a> {
    pub(super) home: &'a Path,
    /// Whether a tagged cache is removed or only counted.
    pub(super) enforcing: bool,
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
    /// Directories discovered but not yet examined, in breadth-first order,
    /// relative to the root.
    pub(super) frontier: VecDeque<PathBuf>,
}

impl<'a> Walk<'a> {
    /// Examine the tree one level at a time, stopping at every directory its
    /// own build tool tagged as regenerable.
    ///
    /// A build tool writes its cache at the TOP of the tree it generates —
    /// that is where the standard puts `CACHEDIR.TAG` — so every candidate is
    /// shallow and everything deep is some tree's contents; level order finds
    /// the caches before it wanders through their neighbours' contents.
    pub(super) fn walk_levels(
        &mut self,
        root_fd: RawFd,
        root: &Path,
        report: &mut CleanupReport,
    ) -> Result<(), JanitorError> {
        while let Some(parent) = self.frontier.pop_front() {
            // Reopen and revalidate every component against the tree as it
            // is now before using a queued directory.
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
                    report.skip_builds("consent_pending", 1);
                    continue;
                }
                Err(_) => {
                    report.skip_builds("entry_replaced", 1);
                    continue;
                }
            };
            self.examine(parent_fd.as_raw_fd(), root, &parent, report)?;
        }
        Ok(())
    }
}
