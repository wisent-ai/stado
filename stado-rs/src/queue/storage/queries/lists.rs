//! Bulk job listings: whole prefixes, id-only walks, the bounded claimable
//! scan, and the grouped view over every lifecycle prefix.

use std::collections::BTreeMap;

use crate::models::Job;
use crate::queue::{listing, Listing, StorageError};

use super::JobStorage;

impl JobStorage {
    /// Parallel-fetch job JSONs under `{prefix}/`. Python
    /// `JobStorage.list_jobs`: `oldest_first` bounds the answer to that many
    /// oldest live jobs, and none bounds it to every job. New callers name
    /// the window with [`listing::list_jobs`] and a [`Listing`].
    pub async fn list_jobs(
        &self,
        prefix: &str,
        oldest_first: usize,
    ) -> Result<Vec<Job>, StorageError> {
        let window = match std::num::NonZeroUsize::new(oldest_first) {
            Some(count) => Listing::Oldest(count),
            None => Listing::Every,
        };
        listing::list_jobs(self, prefix, window).await
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
        for prefix in crate::queue::runs::ALL_PREFIXES {
            result.insert(
                prefix.to_string(),
                listing::list_jobs(self, prefix, Listing::Every).await?,
            );
        }
        Ok(result)
    }
}
