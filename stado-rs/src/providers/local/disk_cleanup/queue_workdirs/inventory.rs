//! The inventory: which workdirs exist, on both roots, within one budget.
//!
//! This is the read-only pass the queue authority is questioned from. It
//! enumerates names only, and it never opens or removes an entry.

use std::collections::BTreeSet;
use std::io;
use std::os::fd::AsRawFd;
use std::path::Path;
use std::time::Instant;

use crate::providers::local::disk_cleanup::queue_workdirs::roots::{job_id, open_work_root_in};
use crate::providers::local::disk_cleanup::queue_workdirs::{LEGACY_WORK_ROOT, WORKDIR_PREFIX};
use crate::providers::local::disk_cleanup::{safefs, JanitorError};

/// Job ids of the exact bounded workdir population this pass may inspect.
///
/// This is a read-only first pass under the janitor lock. It lets the queue
/// authority download documents only for on-disk candidates whose object names
/// are ambiguous, instead of downloading every queue document. A later
/// filesystem change is safe: ids absent here stay on the conservative
/// listing-only keep set.
pub fn candidate_job_ids(
    home: &Path,
    remaining_scan: i64,
    deadline: Instant,
) -> Result<BTreeSet<String>, JanitorError> {
    let mut ids = BTreeSet::new();
    if remaining_scan <= 0 {
        return Ok(ids);
    }
    let (_canonical_root, root_fd, home_device) = match open_work_root_in(home, false) {
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

    let legacy_budget = (remaining_scan / 8)
        .clamp(1, 16)
        .min(remaining_scan.saturating_sub(1));
    let mut canonical_remaining = remaining_scan - legacy_budget;
    for name in safefs::DirEntries::open(root_fd.as_raw_fd())? {
        let name = name?;
        if Instant::now() >= deadline {
            break;
        }
        let text = name.to_string_lossy();
        if !text.starts_with(WORKDIR_PREFIX) {
            continue;
        }
        if let Some(id) = job_id(&text) {
            ids.insert(id.to_string());
        }
        canonical_remaining -= 1;
        if canonical_remaining <= 0 {
            break;
        }
    }

    let legacy_root = Path::new(LEGACY_WORK_ROOT);
    if legacy_root.is_dir() && legacy_budget > 0 {
        // macOS links /tmp to /private/tmp. Resolve only this trusted system
        // root; opens beneath it still refuse symlinked job entries.
        let legacy_fd = safefs::open_dir_path(&legacy_root.canonicalize()?)?;
        let mut legacy_remaining = legacy_budget;
        for name in safefs::DirEntries::open(legacy_fd.as_raw_fd())? {
            let name = name?;
            if Instant::now() >= deadline {
                break;
            }
            let text = name.to_string_lossy();
            if !text.starts_with(WORKDIR_PREFIX) {
                continue;
            }
            if let Some(id) = job_id(&text) {
                ids.insert(id.to_string());
            }
            legacy_remaining -= 1;
            if legacy_remaining <= 0 {
                break;
            }
        }
    }
    Ok(ids)
}
