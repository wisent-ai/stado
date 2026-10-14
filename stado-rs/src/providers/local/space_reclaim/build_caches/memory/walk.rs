//! One build-cache walk at a time on this host.
//!
//! The walk reads the whole home at background priority, which on a full
//! developer host takes many minutes. It used to run inside the janitor's
//! exclusive cleanup lock, so every workload's shared hold — a running job,
//! a product installation — was refused for as long as the walk read the
//! disk, and a host whose pressure the cleaners cannot relieve refused every
//! build it was asked for. The walk now runs under this lock of its own,
//! which keeps two janitors (the agent tick and the health watch) from
//! walking and remembering at once, and takes the exclusive cleanup lock
//! only for the moment it removes one cache.

use std::fs::{File, TryLockError};
use std::path::Path;

use crate::providers::local::space_reclaim::janitor::pass::lock::file::open_lock_at;

/// The walk's own lock, beside the cleanup lock in the janitor's state directory.
const WALK_LOCK: &str = "build-caches.walk.lock";

/// The walk lock, held while the returned file lives; `None` when another
/// janitor is walking now.
pub(in crate::providers::local::space_reclaim) fn exclusive(
    state_dir: &Path,
) -> Result<Option<File>, String> {
    let path = state_dir.join(WALK_LOCK);
    let file = open_lock_at(&path).map_err(|error| {
        format!(
            "the build-cache walk lock {} cannot be opened: {error}",
            path.display()
        )
    })?;
    match file.try_lock() {
        Ok(()) => Ok(Some(file)),
        Err(TryLockError::WouldBlock) => Ok(None),
        Err(TryLockError::Error(error)) => Err(format!(
            "the build-cache walk lock {} cannot be taken: {error}",
            path.display()
        )),
    }
}
