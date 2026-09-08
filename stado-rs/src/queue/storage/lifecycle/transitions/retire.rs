//! Settling a transition: retiring the record and its fenced source once the
//! destination is proven, and deciding the fate of a record whose state this
//! build does not recognize.

use crate::models::Job;
use crate::queue::storage::records::{
    prefix_state, transition_cleaned_state, transition_fence_state, JobTransition,
};
use crate::queue::storage::{transition_path, JobStorage, TRANSITION_RETIRED_STATE};
use crate::queue::{tombstone, StorageError};

impl JobStorage {
    /// Retire only the transition generation this caller finished.
    ///
    /// A plain delete is unsafe: another lifecycle move may replace the
    /// completed record between our final read and delete, and deleting by path
    /// would then erase that newer move. A named retired state keeps the reason
    /// in the durable record, stops later recovery from re-verifying settled
    /// history, and remains replaceable by the next transition.
    async fn retire_transition_record(
        &self,
        completed: &JobTransition,
    ) -> Result<(), StorageError> {
        for _ in 0..16 {
            let Some((mut current, version)) = self.read_job_transition(&completed.job_id).await?
            else {
                return Ok(());
            };
            if current.transition_id != completed.transition_id
                || current.owner != completed.owner
                || current.state == TRANSITION_RETIRED_STATE
            {
                return Ok(());
            }
            if current.state != "completed" {
                return Err(StorageError::StorageConflict(format!(
                    "durable transition {} changed to {} before retirement",
                    completed.transition_id, current.state
                )));
            }
            current.state = TRANSITION_RETIRED_STATE.to_string();
            match self
                .compare_and_swap_text(
                    &transition_path(&completed.job_id),
                    &version,
                    &serde_json::to_string_pretty(&current)?,
                )
                .await
            {
                Ok(_) => return Ok(()),
                Err(StorageError::StorageConflict(_) | StorageError::NotFound(_)) => continue,
                Err(error) => return Err(error),
            }
        }
        Err(StorageError::StorageConflict(format!(
            "durable transition {} remained contended while retiring",
            completed.transition_id
        )))
    }

    async fn retire_transition_source(
        &self,
        transition: &JobTransition,
    ) -> Result<(), StorageError> {
        let source_path = format!("{}/{}.json", transition.from_prefix, transition.job_id);
        let fence_state = transition_fence_state(&transition.transition_id);
        let cleaned_state = transition_cleaned_state(&transition.transition_id);
        for _ in 0..16 {
            let Some(versioned) = self.read_text_versioned(&source_path).await? else {
                return Ok(());
            };
            let mut source = Job::from_json(&versioned.content)?;
            if source.state == cleaned_state {
                return Ok(());
            }
            if source.state != fence_state {
                return Ok(());
            }
            if transition.from_prefix == "queue" {
                // The hot path: every job that leaves the queue passes here.
                // The fenced source still carries the `priority` and
                // `created_at` the marker name was built from, so the name is
                // computable and this is one delete. Walking the index for a
                // matching suffix instead would cost a full listing of the
                // queue per completed job now that every job is indexed.
                self.delete_priority_marker_for(&source).await?;
            }
            source.state = cleaned_state.clone();
            match self
                .compare_and_swap_text(&source_path, &versioned.version, &source.to_json())
                .await
            {
                Ok(_) => return Ok(()),
                Err(StorageError::StorageConflict(_) | StorageError::NotFound(_)) => continue,
                Err(error) => return Err(error),
            }
        }
        Err(StorageError::StorageConflict(format!(
            "source fence for transition {} remained contended",
            transition.transition_id
        )))
    }

    pub(super) async fn finish_completed_transition(
        &self,
        transition: &JobTransition,
    ) -> Result<bool, StorageError> {
        let destination_path = format!("{}/{}.json", transition.to_prefix, transition.job_id);
        let destination = self
            .read_job(&transition.to_prefix, &transition.job_id)
            .await?
            .ok_or_else(|| {
                StorageError::StorageConflict(format!(
                    "completed transition {} has no destination",
                    transition.transition_id
                ))
            })?;
        if crate::queue::submit::immutable_job_projection(&destination)
            != crate::queue::submit::immutable_job_projection(&transition.destination_job)
            || destination.state != prefix_state(&transition.to_prefix)
        {
            return Err(StorageError::StorageConflict(format!(
                "{destination_path} does not match completed transition {}",
                transition.transition_id
            )));
        }
        // Older writers left completed transition records after removing the
        // source. Recovery must retire that settled generation, not replay
        // terminal retention against run history that may already be gone.
        let source_path = format!("{}/{}.json", transition.from_prefix, transition.job_id);
        let source_retired = match self.read_text_versioned(&source_path).await? {
            None => true,
            Some(versioned) => {
                let source = Job::from_json(&versioned.content)?;
                source.job_id == transition.job_id
                    && source.state == transition_cleaned_state(&transition.transition_id)
            }
        };
        if source_retired {
            self.retire_transition_record(transition).await?;
            return Ok(true);
        }
        if crate::queue::runs::TERMINAL_PREFIXES.contains(&transition.to_prefix.as_str()) {
            crate::queue::runs::record_terminal_outcome(self, &destination, &transition.to_prefix)
                .await?;
        }
        self.retire_transition_source(transition).await?;
        tombstone::on_transition(self, &destination, &transition.to_prefix).await;
        // Retire this exact generation rather than deleting by path: the next
        // lifecycle move is allowed to replace a settled record concurrently.
        self.retire_transition_record(transition).await?;
        Ok(true)
    }

    /// Settle a durable record whose state this build does not know.
    ///
    /// This build writes `prepared`, `completed`, `aborted` and the named
    /// retired state. A different value was written outside this state machine.
    /// Raising it into the caller stops far more than the job it describes:
    /// [`Self::claim_queued_job`] recovers before every claim, so one
    /// uninterpretable record ends the agent's whole tick, and on
    /// charless-mac-mini it did — the loop died and restarted every few seconds
    /// for hours while seven pinned jobs waited and every gate read healthy.
    ///
    /// So the world decides instead of the label. A destination that already
    /// carries the promised job in the promised state proves the move
    /// happened, and it is finished exactly as a `completed` record would be.
    /// An intact, unfenced source proves it did not, and the record is
    /// aborted. Only a fenced source with no destination is genuinely
    /// unresolvable, and that one is reported.
    pub(super) async fn settle_unrecognized_transition(
        &self,
        transition: &JobTransition,
    ) -> Result<bool, StorageError> {
        let job_id = transition.job_id.as_str();
        if let Some(destination) = self.read_job(&transition.to_prefix, job_id).await? {
            if crate::queue::submit::immutable_job_projection(&destination)
                == crate::queue::submit::immutable_job_projection(&transition.destination_job)
                && destination.state == prefix_state(&transition.to_prefix)
            {
                self.set_transition_state(transition, "completed").await?;
                return self.finish_completed_transition(transition).await;
            }
        }
        let source_path = format!("{}/{}.json", transition.from_prefix, job_id);
        if let Some(versioned) = self.read_text_versioned(&source_path).await? {
            let source = Job::from_json(&versioned.content)?;
            if source.state != transition_fence_state(&transition.transition_id) {
                self.set_transition_state(transition, "aborted").await?;
                return Ok(false);
            }
        }
        Err(StorageError::StorageConflict(format!(
            "durable transition {} carries unknown state {} and neither side of the move settles it",
            transition.transition_id, transition.state
        )))
    }
}
