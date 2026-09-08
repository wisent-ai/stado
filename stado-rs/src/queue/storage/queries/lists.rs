//! Bulk job listings: whole prefixes, id-only walks, the bounded claimable
//! scan, and the grouped view over every lifecycle prefix.

use std::collections::BTreeMap;

use crate::models::Job;
use crate::queue::{listing, StorageError};

use super::JobStorage;

impl JobStorage {
    /// Parallel-fetch job JSONs under `{prefix}/`. Python
    /// `JobStorage.list_jobs`.
    pub async fn list_jobs(
        &self,
        prefix: &str,
        oldest_first: usize,
    ) -> Result<Vec<Job>, StorageError> {
        listing::list_jobs(self, prefix, oldest_first).await
    }

    /// Every job id under `{prefix}/`, without downloading one document.
    /// See [`listing::list_job_ids`] for why a keep-list must use this and
    /// not [`Self::list_jobs`].
    pub async fn list_job_ids(&self, prefix: &str) -> Result<Vec<String>, StorageError> {
        listing::list_job_ids(self, prefix).await
    }

    /// Priority markers first, then oldest-first, counting only jobs the
    /// caller's own admission rule accepts. See [`listing::JobScan`] for why
    /// the window and the scanning cost are separate quantities.
    pub async fn list_claimable_jobs(
        &self,
        prefix: &str,
        scan: &listing::JobScan<'_>,
    ) -> Result<Vec<Job>, StorageError> {
        listing::list_claimable(self, prefix, scan).await
    }

    /// All jobs grouped by prefix. Python `JobStorage.list_all_jobs`.
    pub async fn list_all_jobs(&self) -> Result<BTreeMap<String, Vec<Job>>, StorageError> {
        let mut result = BTreeMap::new();
        for prefix in [
            "queue",
            "running",
            "completed",
            "uploaded",
            "failed",
            "cancelled",
        ] {
            result.insert(prefix.to_string(), self.list_jobs(prefix, 0).await?);
        }
        Ok(result)
    }
}
