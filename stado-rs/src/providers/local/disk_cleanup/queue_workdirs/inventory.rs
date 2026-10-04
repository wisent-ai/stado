//! The inventory: which workdirs exist, on both roots.
//!
//! This is the read-only pass the queue authority is questioned from. It
//! enumerates names only, and it never opens or removes an entry.

use std::collections::BTreeSet;
use std::io;
use std::os::fd::AsRawFd;
use std::path::Path;

use crate::providers::local::disk_cleanup::queue_workdirs::roots::{job_id, open_cleanup_root};
use crate::providers::local::disk_cleanup::queue_workdirs::{LEGACY_WORK_ROOT, WORKDIR_PREFIX};
use crate::providers::local::disk_cleanup::{safefs, JanitorError};

/// Job ids of the workdir population this pass may inspect.
///
/// This is a read-only first pass under the janitor lock. It lets the queue
/// authority download documents only for on-disk candidates whose object names
/// are ambiguous, instead of downloading every queue document. A later
/// filesystem change is safe: ids absent here stay on the conservative
/// listing-only keep set.
pub fn candidate_job_ids(home: &Path) -> Result<BTreeSet<String>, JanitorError> {
    let mut ids = BTreeSet::new();
    let (_canonical_root, root_fd, home_device) = match open_cleanup_root(home) {
        Ok(opened) => opened,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(ids),
        Err(error) => return Err(error.into()),
    };
    let root_info = safefs::fstat(root_fd.as_raw_fd())?;
    if root_info.st_dev != home_device {
        return Err(JanitorError::os(
            "queue workdir root crosses the home device",
        ));
    }
    let mut collect = |name: &std::ffi::OsStr| {
        let text = name.to_string_lossy();
        if text.starts_with(WORKDIR_PREFIX) {
            if let Some(id) = job_id(&text) {
                ids.insert(id.to_string());
            }
        }
    };
    for name in safefs::DirEntries::open(root_fd.as_raw_fd())? {
        collect(&name?);
    }
    let legacy_root = Path::new(LEGACY_WORK_ROOT);
    if legacy_root.is_dir() {
        // macOS links /tmp to /private/tmp. Resolve only this trusted system
        // root; opens beneath it still refuse symlinked job entries.
        let legacy_fd = safefs::open_dir_path(&legacy_root.canonicalize()?)?;
        for name in safefs::DirEntries::open(legacy_fd.as_raw_fd())? {
            collect(&name?);
        }
    }
    Ok(ids)
}
