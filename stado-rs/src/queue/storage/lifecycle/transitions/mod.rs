//! The durable lifecycle transition protocol: reading and advancing the one
//! typed transition record a job may have ([`workdir`] reads its verdict,
//! [`prepare`] installs one, [`recover`] finishes an abandoned one and
//! [`retire`] settles the record and its fenced source).

use crate::queue::storage::records::{JobTransition, TRANSITION_SCHEMA};
use crate::queue::storage::{transition_path, JobStorage};
use crate::queue::StorageError;

mod prepare;
mod recover;
mod retire;
mod workdir;

impl JobStorage {
    async fn read_job_transition(
        &self,
        job_id: &str,
    ) -> Result<Option<(JobTransition, String)>, StorageError> {
        let Some(versioned) = self.read_text_versioned(&transition_path(job_id)).await? else {
            return Ok(None);
        };
        let transition: JobTransition = serde_json::from_str(&versioned.content)?;
        if transition.schema != TRANSITION_SCHEMA || transition.job_id != job_id {
            return Err(StorageError::Other(format!(
                "invalid durable transition record for {job_id}"
            )));
        }
        Ok(Some((transition, versioned.version)))
    }

    /// Move `expected`'s record to `state`: true once it holds that state,
    /// false when the job's record is no longer `expected`'s generation —
    /// another caller finished that transition and a later one replaced it,
    /// so nothing of `expected` is left for this caller to settle. A write
    /// that loses a race with another writer of the same generation reads
    /// the record again and decides on what it now holds.
    async fn set_transition_state(
        &self,
        expected: &JobTransition,
        state: &str,
    ) -> Result<bool, StorageError> {
        loop {
            let Some((mut transition, version)) =
                self.read_job_transition(&expected.job_id).await?
            else {
                return Err(StorageError::Other(format!(
                    "durable transition {} disappeared",
                    expected.transition_id
                )));
            };
            if transition.transition_id != expected.transition_id
                || transition.owner != expected.owner
            {
                return Ok(false);
            }
            if transition.state == state {
                return Ok(true);
            }
            transition.state = state.to_string();
            match self
                .compare_and_swap_text(
                    &transition_path(&expected.job_id),
                    &version,
                    &serde_json::to_string_pretty(&transition)?,
                )
                .await
            {
                Ok(_) => return Ok(true),
                Err(StorageError::StorageConflict(_)) => continue,
                Err(error) => return Err(error),
            }
        }
    }
}
