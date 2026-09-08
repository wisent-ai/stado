//! Recovery: finishing whatever the one durable transition record for a job
//! promised, whoever prepared it and whether or not that owner is still alive.

use crate::models::Job;
use crate::queue::storage::records::{
    prefix_state, sha256_hex, transition_cleaned_state, transition_fence_state,
    TRANSITION_CLEANED_PREFIX,
};
use crate::queue::storage::{JobStorage, TRANSITION_RETIRED_STATE};
use crate::queue::StorageError;

impl JobStorage {
    /// Recover or finish the single durable lifecycle transition for a job.
    /// Recovery is ownership-independent: the persisted intent and source
    /// version are the fence, so any caller can complete an abandoned owner.
    pub async fn recover_job_transition(&self, job_id: &str) -> Result<bool, StorageError> {
        let Some((transition, _)) = self.read_job_transition(job_id).await? else {
            return Ok(false);
        };
        if transition.state == TRANSITION_RETIRED_STATE {
            return Ok(false);
        }
        if transition.state == "aborted" {
            // `aborted` is a decision made from an earlier view of the two
            // sides, not a terminal fact. A later placement rewrite can make
            // that view stale while the destination remains the completed
            // move. Upgrade only when the destination matches and the source
            // is absent, fenced, or already cleaned for this generation.
            let Some(destination) = self.read_job(&transition.to_prefix, job_id).await? else {
                return Ok(false);
            };
            if crate::queue::submit::immutable_job_projection(&destination)
                != crate::queue::submit::immutable_job_projection(&transition.destination_job)
                || destination.state != prefix_state(&transition.to_prefix)
            {
                return Ok(false);
            }
            let source_path = format!("{}/{}.json", transition.from_prefix, job_id);
            if let Some(versioned) = self.read_text_versioned(&source_path).await? {
                let source = Job::from_json(&versioned.content)?;
                let fence_state = transition_fence_state(&transition.transition_id);
                let cleaned_state = transition_cleaned_state(&transition.transition_id);
                if source.state != fence_state && source.state != cleaned_state {
                    return Ok(false);
                }
            }
            self.set_transition_state(&transition, "completed").await?;
            return self.finish_completed_transition(&transition).await;
        }
        let source_path = format!("{}/{}.json", transition.from_prefix, job_id);
        let destination_path = format!("{}/{}.json", transition.to_prefix, job_id);
        let fence_state = transition_fence_state(&transition.transition_id);

        if transition.state == "completed" {
            return self.finish_completed_transition(&transition).await;
        }
        if transition.state != "prepared" {
            return self.settle_unrecognized_transition(&transition).await;
        }

        match self.read_text_versioned(&source_path).await? {
            Some(versioned)
                if versioned.version == transition.source_version
                    && sha256_hex(versioned.content.as_bytes()) == transition.source_digest =>
            {
                let mut source = Job::from_json(&versioned.content)?;
                source.state = fence_state.clone();
                match self
                    .compare_and_swap_text(&source_path, &versioned.version, &source.to_json())
                    .await
                {
                    Ok(_) => {}
                    Err(StorageError::StorageConflict(_) | StorageError::NotFound(_)) => {
                        return Ok(false)
                    }
                    Err(error) => return Err(error),
                }
            }
            Some(versioned) => {
                let source = Job::from_json(&versioned.content)?;
                if source.state != fence_state {
                    self.set_transition_state(&transition, "aborted").await?;
                    return Ok(false);
                }
            }
            None => {
                let Some(destination) = self.read_job(&transition.to_prefix, job_id).await? else {
                    return Err(StorageError::StorageConflict(format!(
                        "transition {} lost both source and destination",
                        transition.transition_id
                    )));
                };
                if crate::queue::submit::immutable_job_projection(&destination)
                    != crate::queue::submit::immutable_job_projection(&transition.destination_job)
                    || destination.state != prefix_state(&transition.to_prefix)
                {
                    return Err(StorageError::StorageConflict(format!(
                        "{destination_path} does not match transition {}",
                        transition.transition_id
                    )));
                }
                self.set_transition_state(&transition, "completed").await?;
                return self.finish_completed_transition(&transition).await;
            }
        }

        let mut installed_destination = None;
        for _ in 0..16 {
            let existing_versioned = self.read_text_versioned(&destination_path).await?;
            let Some(existing_versioned) = existing_versioned else {
                if self
                    .backend
                    .upload_text_if_absent(&destination_path, &transition.destination_job.to_json())
                    .await?
                {
                    installed_destination = Some(transition.destination_job.clone());
                    break;
                }
                continue;
            };
            let existing = Job::from_json(&existing_versioned.content)?;
            if existing.state.starts_with(TRANSITION_CLEANED_PREFIX) {
                match self
                    .compare_and_swap_text(
                        &destination_path,
                        &existing_versioned.version,
                        &transition.destination_job.to_json(),
                    )
                    .await
                {
                    Ok(_) => {
                        installed_destination = Some(transition.destination_job.clone());
                        break;
                    }
                    Err(StorageError::StorageConflict(_) | StorageError::NotFound(_)) => continue,
                    Err(error) => return Err(error),
                }
            }
            if crate::queue::submit::immutable_job_projection(&existing)
                != crate::queue::submit::immutable_job_projection(&transition.destination_job)
                || existing.state != prefix_state(&transition.to_prefix)
            {
                return Err(StorageError::StorageConflict(format!(
                    "{destination_path} conflicts with transition {}",
                    transition.transition_id
                )));
            }
            if transition.destination_version.as_deref()
                == Some(existing_versioned.version.as_str())
                && existing.to_json() != transition.destination_job.to_json()
            {
                match self
                    .compare_and_swap_text(
                        &destination_path,
                        &existing_versioned.version,
                        &transition.destination_job.to_json(),
                    )
                    .await
                {
                    Ok(_) => {
                        installed_destination = Some(transition.destination_job.clone());
                        break;
                    }
                    Err(StorageError::StorageConflict(_) | StorageError::NotFound(_)) => continue,
                    Err(error) => return Err(error),
                }
            } else {
                installed_destination = Some(existing);
                break;
            }
        }
        let destination = installed_destination.ok_or_else(|| {
            StorageError::StorageConflict(format!(
                "{destination_path} remained contended during transition {}",
                transition.transition_id
            ))
        })?;
        self.backend
            .set_metadata(&destination_path, &Self::job_metadata(&destination))
            .await?;
        if transition.to_prefix == "queue" {
            // Anything re-entering the queue is indexed, whatever its
            // priority: a requeued job with priority 0 that carried no marker
            // would be invisible to a listing that walks only the index.
            self.write_priority_marker(&destination).await?;
        }
        self.set_transition_state(&transition, "completed").await?;
        self.finish_completed_transition(&transition).await
    }
}
