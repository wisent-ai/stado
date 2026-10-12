//! The verdict on one tagged cache: the reported bytes and — in an
//! enforcing pass only — the reclamation itself.

use std::ffi::OsStr;
use std::os::fd::RawFd;
use std::path::Path;

use nix::sys::stat::FileStat;

use crate::providers::local::disk_cleanup::build_caches::remove::remove_tree;
use crate::providers::local::disk_cleanup::build_caches::walk::Walk;
use crate::providers::local::disk_cleanup::{free_bytes, CleanupReport, JanitorError};

impl<'a> Walk<'a> {
    /// Judge one directory whose tag is valid, and — when enforcing — remove
    /// it. Never descends: a cache nested in a cache needs no special case,
    /// because the parent is reported and removed whole, so the child must
    /// not be counted a second time.
    ///
    /// The bytes come from the walk's memory when the root directory
    /// (`info`, at `absolute`) is the one an earlier pass measured — counted
    /// under `measurement_remembered` — and from reading the tree otherwise.
    pub(super) fn judge(
        &mut self,
        parent_fd: RawFd,
        name: &OsStr,
        dir_fd: RawFd,
        info: &FileStat,
        absolute: &Path,
        report: &mut CleanupReport,
    ) -> Result<(), JanitorError> {
        report.builds.eligible_items += 1;
        let remembered_before = self.memory.remembered();
        let expected = match self.memory.recall(absolute, info) {
            Some(bytes) => {
                // Counted by the entries the memory answered, with the bytes
                // it answered for, so the report says what was not re-read.
                let answered = self.memory.remembered().saturating_sub(remembered_before);
                report.skip_builds("measurement_remembered", answered as i64);
                *report
                    .builds
                    .skipped_bytes
                    .entry("measurement_remembered".to_string())
                    .or_default() += bytes;
                bytes
            }
            None => match self.tree_bytes(dir_fd, 0) {
                Ok(bytes) => {
                    self.memory.measured(absolute, info, bytes);
                    bytes
                }
                Err(mut error) => {
                    error.message = format!("cannot measure cache {name:?}: {}", error.message);
                    report.add_error("build_caches", &error);
                    return Ok(());
                }
            },
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
                self.memory.removed(absolute);
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
