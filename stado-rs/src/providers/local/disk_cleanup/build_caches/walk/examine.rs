//! One frontier directory's worth of children: the symlink, ownership,
//! device, identity and reserved-root refusals, then either a verdict on a
//! tagged cache or a place on the next level's queue.

use std::ffi::OsStr;
use std::os::fd::{AsRawFd, RawFd};
use std::path::Path;
use std::time::Instant;

use crate::providers::local::disk_cleanup::build_caches::walk::tag::Tag;
use crate::providers::local::disk_cleanup::build_caches::walk::{Progress, Walk};
use crate::providers::local::disk_cleanup::build_caches::{entry_names, same_object};
use crate::providers::local::disk_cleanup::{
    euid, ifmt, safefs, CleanupReport, JanitorError, IFDIR, IFLNK,
};

impl<'a> Walk<'a> {
    /// Examine every child of one frontier directory: judge the ones their
    /// build tool tagged, and hand the rest to the next level.
    pub(super) fn examine(
        &mut self,
        parent_fd: RawFd,
        root: &Path,
        parent: &Path,
        report: &mut CleanupReport,
    ) -> Result<Progress, JanitorError> {
        let names = match entry_names(parent_fd) {
            Ok(names) => names,
            Err(_) => {
                report.skip_builds("stat_failed", 1);
                return Ok(Progress::Continue);
            }
        };
        let first = self
            .next_child
            .take()
            .and_then(|path| path.file_name().map(OsStr::to_os_string))
            .unwrap_or_default();
        for name in names.range(first..) {
            let relative = parent.join(name);
            if Instant::now() >= self.deadline {
                report.caps.deadline = true;
                report.skip_builds("scan_deadline", 1);
                self.next_child = Some(relative);
                return Ok(Progress::Halt);
            }
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
            if self.reserved.iter().any(|item| absolute.starts_with(item)) {
                report.skip_builds("reserved_or_hidden", 1);
                continue;
            }
            if let Progress::Halt = self.charge(report) {
                self.next_child = Some(relative);
                return Ok(Progress::Halt);
            }
            let child = match safefs::open_dir_at(parent_fd, name) {
                Ok(child) => child,
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
            let guards_reserved = self.reserved.iter().any(|item| item.starts_with(&absolute));
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
                    self.frontier.push_back(relative.into());
                }
                // Tagged caches remain indivisible candidates, never parents
                // whose contents can be independently selected for deletion.
                Tag::Signed => {
                    match self.judge(parent_fd, name, child.as_raw_fd(), &child_info, report) {
                        Ok(Progress::Continue) => {}
                        result => {
                            self.next_child = Some(relative);
                            return result;
                        }
                    }
                }
                Tag::Unsigned => {
                    report.skip_builds("untagged", 1);
                    self.frontier.push_back(relative.into());
                }
                Tag::Absent => self.frontier.push_back(relative.into()),
            }
        }
        Ok(Progress::Continue)
    }
}
