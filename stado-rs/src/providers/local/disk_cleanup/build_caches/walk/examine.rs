//! One frontier directory's worth of children: the symlink, ownership,
//! device, identity and reserved-root refusals, then either a verdict on a
//! tagged cache or a place on the next level's queue.

use std::ffi::OsStr;
use std::os::fd::{AsRawFd, RawFd};
use std::path::Path;

use crate::providers::local::disk_cleanup::build_caches::walk::tag::Tag;
use crate::providers::local::disk_cleanup::build_caches::walk::Walk;
use crate::providers::local::disk_cleanup::build_caches::{entry_names, same_object};
use crate::providers::local::disk_cleanup::consent::{self, Gated};
use crate::providers::local::disk_cleanup::{
    euid, ifmt, safefs, CleanupReport, JanitorError, IFDIR, IFLNK,
};

/// The suffixes macOS gives a bundle: a directory the Finder, the installer
/// and the operating system treat as one file. A build tool never tags one.
const BUNDLE_SUFFIXES: [&str; 4] = [".app", ".framework", ".bundle", ".xcassets"];

fn is_bundle(name: &OsStr) -> bool {
    let Some(name) = name.to_str() else {
        return false;
    };
    BUNDLE_SUFFIXES.iter().any(|suffix| name.ends_with(suffix))
}

impl<'a> Walk<'a> {
    /// Examine every child of one frontier directory: judge the ones their
    /// build tool tagged, and hand the rest to the next level.
    pub(super) fn examine(
        &mut self,
        parent_fd: RawFd,
        root: &Path,
        parent: &Path,
        report: &mut CleanupReport,
    ) -> Result<(), JanitorError> {
        let names = match entry_names(parent_fd) {
            Ok(names) => names,
            Err(_) => {
                report.skip_builds("stat_failed", 1);
                return Ok(());
            }
        };
        for name in &names {
            let relative = parent.join(name);
            let info = match safefs::fstatat_nofollow(parent_fd, name) {
                Ok(info) => info,
                Err(_) => {
                    report.skip_builds("stat_failed", 1);
                    continue;
                }
            };
            let kind = ifmt(info.st_mode as u32);
            if kind == IFLNK {
                report.skip_builds("symlink_not_followed", 1);
                continue;
            }
            if kind != IFDIR {
                continue;
            }
            let absolute = root.join(&relative);
            // A macOS bundle is one opaque item to the person who installed
            // it, and no build tool writes a tagged cache inside one. A walk
            // that descends anyway spends its time on an application's
            // payload and finds nothing.
            if is_bundle(name) {
                report.skip_builds("application_bundle", 1);
                continue;
            }
            // Before the descriptor is opened: on macOS, opening it is what
            // raises the consent dialog this cleaner has no business raising.
            if self.privacy.iter().any(|item| absolute.starts_with(item)) {
                report.skip_builds("privacy_protected", 1);
                continue;
            }
            if self.reserved.iter().any(|item| absolute.starts_with(item)) {
                report.skip_builds("reserved_or_hidden", 1);
                continue;
            }
            report.builds.scanned_items += 1;
            let child = match consent::open_dir_at(&self.gated, parent_fd, name, &absolute) {
                Ok(Gated::Opened(child)) => child,
                Ok(Gated::Pending) => {
                    // The consent question is with the person at the keyboard.
                    // This folder waits for the answer; the walk goes on with
                    // its siblings, so one unanswered dialog never stops the
                    // rule from reaching the rest of the home.
                    report.skip_builds("consent_pending", 1);
                    continue;
                }
                Err(exc)
                    if matches!(
                        exc.raw_os_error(),
                        Some(nix::libc::ELOOP) | Some(nix::libc::ENOTDIR)
                    ) =>
                {
                    report.skip_builds("entry_replaced", 1);
                    continue;
                }
                Err(_) => {
                    report.skip_builds("stat_failed", 1);
                    continue;
                }
            };
            let child_info = match safefs::fstat(child.as_raw_fd()) {
                Ok(info) => info,
                Err(_) => {
                    report.skip_builds("stat_failed", 1);
                    continue;
                }
            };
            if !same_object(&child_info, &info) {
                report.skip_builds("entry_replaced", 1);
                continue;
            }
            if child_info.st_uid != euid() || child_info.st_dev != self.root_dev {
                report.skip_builds("unsafe_owner_or_device", 1);
                continue;
            }
            let guards_reserved = self
                .reserved
                .iter()
                .chain(self.privacy.iter())
                .any(|item| item.starts_with(&absolute));
            let tag = match self.read_tag(child.as_raw_fd()) {
                Ok(tag) => tag,
                Err(_) => {
                    report.skip_builds("stat_failed", 1);
                    Tag::Absent
                }
            };
            match tag {
                Tag::Signed if guards_reserved => {
                    report.skip_builds("reserved_or_hidden", 1);
                    self.frontier.push_back(relative);
                }
                // Tagged caches remain indivisible candidates, never parents
                // whose contents can be independently selected for deletion.
                Tag::Signed => self.judge(parent_fd, name, child.as_raw_fd(), report)?,
                Tag::Unsigned => {
                    report.skip_builds("untagged", 1);
                    self.frontier.push_back(relative);
                }
                Tag::Absent => self.frontier.push_back(relative),
            }
        }
        Ok(())
    }
}
