//! The shared-mode hold one live workload keeps on the cleanup lock.

use std::fs::File;
use std::path::{Path, PathBuf};

use crate::providers::local::disk_cleanup::janitor::pass::lock::file::{
    io_code, lock_contended, open_lock,
};
use crate::providers::local::disk_cleanup::janitor::pass::lock::holds;
use crate::providers::local::disk_cleanup::janitor::pass::lock::{ensure_state_dir, secure_home};
use crate::providers::local::disk_cleanup::janitor::state::error::JanitorError;

/// The agent's word for "a standalone cleanup holds the exclusive lock, so
/// no workload can take its shared hold": published as `admission_reason`
/// in the capacity broadcast and as `disk_cleanup_admission` beside a claim
/// that stopped for it, and read back verbatim by `stado host gates`.
pub const CLEANUP_IN_PROGRESS: &str = "cleanup_in_progress";

/// The agent's word for "the workload lock could not be probed at all":
/// the state directory or the lock file refused the open. A claim stops for
/// this exactly as it stops for [`CLEANUP_IN_PROGRESS`], so the publication
/// carries it as the `admission_reason`, with the error code appended under
/// `disk_cleanup_admission`.
pub const CLEANUP_LOCK_ERROR: &str = "cleanup_lock_error";

/// A shared-mode hold on the cleanup lock for one live workload
/// (Python's opaque `int` handle from `acquire_workload_lock`), with the
/// record that names it to a janitor that finds the lock busy.
#[derive(Debug)]
pub struct WorkloadLock {
    file: Option<File>,
    record: Option<PathBuf>,
}

impl WorkloadLock {
    fn settle(&mut self) -> Option<File> {
        if let Some(record) = self.record.take() {
            let _ = std::fs::remove_file(record);
        }
        self.file.take()
    }
}

impl Drop for WorkloadLock {
    fn drop(&mut self) {
        if let Some(file) = self.settle() {
            let _ = fs2::FileExt::unlock(&file);
        }
    }
}

/// Python `acquire_workload_lock` at an explicit home (test seam). `holder`
/// names the work the hold stands for (a job id) in the record a busy janitor
/// reads. While the janitor has asked for its turn, no new hold is given, as
/// while it holds the lock exclusively.
pub fn acquire_workload_lock_in(
    home: &Path,
    holder: &str,
) -> Result<Option<WorkloadLock>, JanitorError> {
    let state_dir = ensure_state_dir(&secure_home(home)?)?;
    if holds::turn_requested(&state_dir) {
        return Ok(None);
    }
    let file = open_lock(&state_dir)?;
    match fs2::FileExt::try_lock_shared(&file) {
        Ok(()) => {
            let mut lock = WorkloadLock {
                file: Some(file),
                record: None,
            };
            lock.record = Some(holds::record(&state_dir, holder)?);
            Ok(Some(lock))
        }
        Err(exc) if lock_contended(&exc) => Ok(None),
        Err(exc) => Err(exc.into()),
    }
}

/// Acquire the cleanup lock in shared mode for one live workload.
///
/// The returned opaque handle must be retained until the workload has
/// fully left its slot, then passed to [`release_workload_lock`]. `None`
/// means a standalone cleanup currently owns the exclusive lock, or has asked
/// for its turn, so admission must be retried later.
pub fn acquire_workload_lock(holder: &str) -> Result<Option<WorkloadLock>, JanitorError> {
    acquire_workload_lock_in(&crate::config_file::expand_tilde("~"), holder)
}

/// Release a handle returned by [`acquire_workload_lock`]
/// (Python `release_workload_lock`: flock UN, then close; Drop provides the
/// same explicit unlock backstop when a caller releases by scope).
pub fn release_workload_lock(mut lock: WorkloadLock, log_fn: &mut dyn FnMut(&str)) {
    let Some(file) = lock.settle() else {
        return;
    };
    if let Err(exc) = fs2::FileExt::unlock(&file) {
        log_fn(&format!(
            "disk cleanup workload lock release failed: {}",
            io_code(&exc)
        ));
    }
}
