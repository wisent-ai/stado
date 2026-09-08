//! The authoritative lifecycle verdict a disk janitor needs about one queue
//! workdir candidate, read entirely from durable documents.

use crate::models::Job;
use crate::queue::storage::records::{
    cleaned_transition_id, prefix_state, TRANSITION_FENCE_PREFIX,
};
use crate::queue::storage::{JobStorage, WorkdirJobState, TRANSITION_RETIRED_STATE};
use crate::queue::StorageError;

impl JobStorage {
    /// Resolve one workdir candidate without exposing transition protocol
    /// internals to the janitor.
    ///
    /// A terminal verdict requires this job's current typed transition to be
    /// retired, its queue/running source documents to be the matching cleaned
    /// sentinel (or absent), and its verified destination to remain in the
    /// recorded terminal prefix. Every live, fenced, malformed, mismatched, or
    /// otherwise unknown document is retained.
    pub(crate) async fn workdir_job_state(
        &self,
        job_id: &str,
    ) -> Result<WorkdirJobState, StorageError> {
        let Some((transition, _)) = self.read_job_transition(job_id).await? else {
            return Ok(WorkdirJobState::Unknown);
        };
        if transition.state != TRANSITION_RETIRED_STATE
            || !crate::queue::runs::TERMINAL_PREFIXES.contains(&transition.to_prefix.as_str())
            || transition.destination_job.job_id != job_id
            || transition.destination_job.state != prefix_state(&transition.to_prefix)
        {
            return Ok(WorkdirJobState::Unknown);
        }
        let expected_projection =
            crate::queue::submit::immutable_job_projection(&transition.destination_job);

        for prefix in ["queue", "running"] {
            let Some(versioned) = self
                .read_text_versioned(&format!("{prefix}/{job_id}.json"))
                .await?
            else {
                continue;
            };
            let job = match Job::from_json(&versioned.content) {
                Ok(job) if job.job_id == job_id => job,
                _ => return Ok(WorkdirJobState::Unknown),
            };
            if job.state == prefix_state(prefix) {
                return Ok(WorkdirJobState::Live);
            }
            if crate::queue::submit::immutable_job_projection(&job) != expected_projection {
                return Ok(WorkdirJobState::Unknown);
            }
            let Some(cleaned_id) = cleaned_transition_id(&job.state) else {
                return Ok(WorkdirJobState::Unknown);
            };
            if prefix == transition.from_prefix && cleaned_id != transition.transition_id {
                return Ok(WorkdirJobState::Unknown);
            }
        }

        let mut terminal_destination = false;
        for prefix in crate::queue::runs::TERMINAL_PREFIXES {
            let Some(versioned) = self
                .read_text_versioned(&format!("{prefix}/{job_id}.json"))
                .await?
            else {
                continue;
            };
            let job = match Job::from_json(&versioned.content) {
                Ok(job) if job.job_id == job_id => job,
                _ => return Ok(WorkdirJobState::Unknown),
            };
            if job.state.starts_with(TRANSITION_FENCE_PREFIX) {
                return Ok(WorkdirJobState::Unknown);
            }
            if crate::queue::submit::immutable_job_projection(&job) != expected_projection {
                return Ok(WorkdirJobState::Unknown);
            }
            if prefix == transition.to_prefix && job.state == prefix_state(prefix) {
                terminal_destination = true;
            } else {
                let Some(cleaned_id) = cleaned_transition_id(&job.state) else {
                    return Ok(WorkdirJobState::Unknown);
                };
                if prefix == transition.from_prefix && cleaned_id != transition.transition_id {
                    return Ok(WorkdirJobState::Unknown);
                }
            }
        }

        Ok(if terminal_destination {
            WorkdirJobState::Terminal
        } else {
            WorkdirJobState::Unknown
        })
    }
}
