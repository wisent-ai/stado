//! The verdict on one tagged cache: the reported bytes and — in an
//! enforcing pass only — the reclamation itself.

use std::ffi::OsStr;
use std::os::fd::RawFd;

use crate::providers::local::disk_cleanup::build_caches::remove::remove_tree;
use crate::providers::local::disk_cleanup::build_caches::walk::Walk;
use crate::providers::local::disk_cleanup::{free_bytes, CleanupReport, JanitorError};

impl<'a> Walk<'a> {
    /// Judge one directory whose tag is valid, and — when enforcing — remove
    /// it. Never descends: a cache nested in a cache needs no special case,
    /// because the parent is reported and removed whole, so the child must
    /// not be counted a second time.
    pub(super) fn judge(
        &mut self,
        parent_fd: RawFd,
        name: &OsStr,
        dir_fd: RawFd,
        report: &mut CleanupReport,
    ) -> Result<(), JanitorError> {
        report.builds.eligible_items += 1;
        let expected = match self.tree_bytes(dir_fd, 0) {
            Ok(bytes) => bytes,
            Err(mut error) => {
                error.message = format!("cannot measure cache {name:?}: {}", error.message);
                report.add_error("build_caches", &error);
                return Ok(());
            }
        };
        report.builds.expected_bytes += expected;
        if !self.enforcing {
            return Ok(());
        }
        let before = free_bytes(self.home)?;
        match remove_tree(parent_fd, name, dir_fd, self.root_dev) {
            Ok(()) => {
                let after = free_bytes(self.home)?;
                report.builds.actual_free_delta_bytes += (after - before).max(0);
                report.builds.deleted_items += 1;
            }
            // A partially removed tree is reported, not retried: whatever
            // refused (an unwritable subdirectory, a vanished entry, a
            // replaced one) will be judged again next pass on what is left.
            Err(exc) => report.add_error("build_caches", &exc),
        }
        Ok(())
    }
}
