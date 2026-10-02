//! The verdict on one tagged cache: the age gate, the reported bytes, the
//! per-pass item, byte and free-space caps, and — in `enforce` only — the
//! reclamation itself.

use std::ffi::OsStr;
use std::os::fd::RawFd;

use nix::sys::stat::FileStat;

use crate::providers::local::disk_cleanup::build_caches::remove::remove_tree;
use crate::providers::local::disk_cleanup::build_caches::walk::{Progress, Walk};
use crate::providers::local::disk_cleanup::{free_bytes, CleanupReport, JanitorError, GIB};

impl<'a> Walk<'a> {
    /// Judge one directory whose tag is valid, and — in `enforce` — remove
    /// it. Never descends: a cache nested in a cache needs no special case,
    /// because the parent is reported and removed whole, so the child must
    /// not be counted a second time.
    pub(super) fn judge(
        &mut self,
        parent_fd: RawFd,
        name: &OsStr,
        dir_fd: RawFd,
        info: &FileStat,
        report: &mut CleanupReport,
    ) -> Result<Progress, JanitorError> {
        if !self.old_enough(info) {
            report.skip_builds("too_young", 1);
            return Ok(Progress::Continue);
        }
        report.builds.eligible_items += 1;
        let expected = match self.tree_bytes(dir_fd, 0) {
            Ok(bytes) => bytes,
            Err(mut error) => {
                error.message = format!("cannot measure cache {name:?}: {}", error.message);
                report.add_error("build_caches", &error);
                return Ok(Progress::Continue);
            }
        };
        report.builds.expected_bytes += expected;
        if self.policy.mode != "enforce" {
            return Ok(Progress::Continue);
        }
        if report.builds.deleted_items >= self.policy.max_items_per_pass {
            report.caps.items = true;
            report.skip_builds("item_cap", 1);
            return Ok(Progress::Continue);
        }
        if self.deleted_bytes >= self.policy.max_bytes_per_pass {
            report.caps.bytes = true;
            report.skip_builds("byte_cap", 1);
            return Ok(Progress::Continue);
        }
        // Enough was recovered: stop deleting. The pass computes the
        // outcome from free space afterwards, but nothing above this call
        // stops the scan, so the cleaner has to.
        if free_bytes(self.home)? >= self.policy.target_free_gb * GIB {
            return Ok(Progress::Halt);
        }
        let before = free_bytes(self.home)?;
        match remove_tree(parent_fd, name, dir_fd, self.root_dev) {
            Ok(()) => {
                let after = free_bytes(self.home)?;
                report.builds.actual_free_delta_bytes += (after - before).max(0);
                report.builds.deleted_items += 1;
                self.deleted_bytes += expected;
            }
            // A partially removed tree is reported, not retried: whatever
            // refused (an unwritable subdirectory, a vanished entry, a
            // replaced one) will be judged again next pass on what is left.
            Err(exc) => report.add_error("build_caches", &exc),
        }
        Ok(Progress::Continue)
    }
}
