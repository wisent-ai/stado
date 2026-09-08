//! Reads: one job document here, the priority-index markers in [`markers`],
//! the bulk and claimable listings in [`lists`], and the per-job script and
//! status objects in [`artifacts`].

use crate::models::Job;
use crate::queue::StorageError;

use super::{is_transition_sentinel_state, JobStorage};

mod artifacts;
mod lists;
mod markers;

impl JobStorage {
    /// Read a job blob; `None` when absent. Corrupt JSON propagates as an
    /// error (the Python code strict-raises since the listing extraction).
    pub async fn read_job(&self, prefix: &str, job_id: &str) -> Result<Option<Job>, StorageError> {
        let Some(data) = self
            .backend
            .download_text(&format!("{prefix}/{job_id}.json"))
            .await?
        else {
            return Ok(None);
        };
        let job = Job::from_json(&data)?;
        if is_transition_sentinel_state(&job.state) {
            return Ok(None);
        }
        Ok(Some(job))
    }
}
