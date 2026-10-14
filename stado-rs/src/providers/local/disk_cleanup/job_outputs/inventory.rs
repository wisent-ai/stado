//! The candidate population of the job-outputs cleaner: the job ids that
//! have a `status/<job_id>/output/` directory on this host. The queue is
//! asked about exactly these names, and only the ones it positively lists as
//! terminal are ever touched.

use std::collections::BTreeSet;
use std::path::Path;

use crate::providers::local::disk_cleanup::JanitorError;

/// Every job id with an output directory under any of `status_roots`.
pub fn candidate_job_ids(
    status_roots: &[std::path::PathBuf],
) -> Result<BTreeSet<String>, JanitorError> {
    let mut ids = BTreeSet::new();
    for root in status_roots {
        collect_from(root, &mut ids)?;
    }
    Ok(ids)
}

/// The ids one root holds.
fn collect_from(status_root: &Path, ids: &mut BTreeSet<String>) -> Result<(), JanitorError> {
    let entries = match std::fs::read_dir(status_root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(JanitorError::os(&format!(
                "list job outputs {}: {error}",
                status_root.display()
            )))
        }
    };
    for entry in entries {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        if !entry.path().join(super::OUTPUT_DIR).is_dir() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') || name.contains('/') {
            continue;
        }
        ids.insert(name);
    }
    Ok(())
}
