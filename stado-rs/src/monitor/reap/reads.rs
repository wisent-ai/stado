//! Liveness reads of a lifecycle store: the canonical snapshot path, the
//! required-content read, and the terminal-job probe every classifier uses to
//! tell live work from retired work.

use crate::queue::{JobStorage, StorageError};

pub(super) fn snapshot_full_path(relative: &str) -> String {
    format!("ecosystem/probierz/{relative}")
}

pub(super) async fn required_snapshot_text(
    store: &JobStorage,
    path: &str,
) -> Result<String, StorageError> {
    store
        .download_text(path)
        .await?
        .ok_or_else(|| StorageError::NotFound(path.to_string()))
}

pub(super) async fn terminal_snapshot_present(
    store: &JobStorage,
    job_id: &str,
) -> Result<bool, StorageError> {
    for prefix in crate::queue::runs::TERMINAL_PREFIXES {
        if store
            .read_job(prefix, job_id)
            .await?
            .is_some_and(|job| job.job_id == job_id && job.state == prefix)
        {
            return Ok(true);
        }
    }
    Ok(false)
}
