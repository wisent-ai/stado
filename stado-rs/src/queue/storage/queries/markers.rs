//! Priority-index markers: the facade's delegates onto `queue::listing`.

use crate::models::Job;
use crate::queue::{listing, StorageError};

use super::JobStorage;

impl JobStorage {
    // ---- delegates to queue/listing/ (priority markers + bulk fetch) ----

    /// Index entry for a queued job (`queue_priority/` marker).
    pub async fn write_priority_marker(&self, job: &Job) -> Result<(), StorageError> {
        listing::write_marker(self, job).await
    }

    /// Drop the marker this job names, in one delete.
    pub async fn delete_priority_marker_for(&self, job: &Job) -> Result<(), StorageError> {
        listing::delete_marker_for(self, job).await
    }

    /// Repair path: drop every marker naming `job_id` by walking the index,
    /// except `keep` when the caller has already written the current one.
    /// Only for the cases where the marker's key is not derivable from the
    /// job — an orphan, or a key superseded by a priority change.
    pub async fn repair_priority_markers(
        &self,
        job_id: &str,
        keep: Option<&str>,
    ) -> Result<(), StorageError> {
        listing::delete_markers_scanning(self, job_id, keep).await
    }
}
