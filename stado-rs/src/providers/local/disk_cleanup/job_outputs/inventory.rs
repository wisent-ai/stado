//! The bounded candidate population of the job-outputs cleaner: the job ids
//! that have a `status/<job_id>/output/` directory on this host. The queue is
//! asked about exactly these names, and only the ones it positively lists as
//! terminal are ever touched.

use std::collections::BTreeSet;
use std::path::Path;
use std::time::Instant;

use crate::providers::local::disk_cleanup::JanitorError;

/// Every job id with an output directory under any of `status_roots`,
/// at most `remaining_scan` of them.
pub fn candidate_job_ids(
    status_roots: &[std::path::PathBuf],
    remaining_scan: i64,
    deadline: Instant,
) -> Result<BTreeSet<String>, JanitorError> {
    let mut ids = BTreeSet::new();
    let mut budget = remaining_scan;
    for root in status_roots {
        if budget <= 0 || Instant::now() >= deadline {
            break;
        }
        budget -= collect_from(root, budget, deadline, &mut ids)?;
    }
    Ok(ids)
}

/// The ids one root holds, and how much of the budget the walk spent.
fn collect_from(
    status_root: &Path,
    remaining_scan: i64,
    deadline: Instant,
    ids: &mut BTreeSet<String>,
) -> Result<i64, JanitorError> {
    if remaining_scan <= 0 {
        return Ok(0);
    }
    let entries = match std::fs::read_dir(status_root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(error) => {
            return Err(JanitorError::os(&format!(
                "list job outputs {}: {error}",
                status_root.display()
            )))
        }
    };
    let mut spent = 0i64;
    let mut budget = remaining_scan;
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
        spent += 1;
        budget -= 1;
        if budget <= 0 || Instant::now() >= deadline {
            break;
        }
    }
    Ok(spent)
}
