//! First admission: creating a queued job exactly once, and repairing the
//! admission metadata of a create that lost its race.

use crate::models::Job;
use crate::queue::storage::{transition_path, JobStorage};
use crate::queue::StorageError;

impl JobStorage {
    // ---- job operations ----

    /// Create a queued job exactly once. Existing content is never overwritten;
    /// callers must read it back and verify its submission identity.
    pub async fn create_queued_job_if_absent(&self, job: &Job) -> Result<bool, StorageError> {
        // This is first admission, not lifecycle recovery. A crash may leave
        // the durable run entry at `enqueuing` after this job has already
        // moved out of every visible lifecycle prefix. Settle any active move
        // first, then let durable transition history fence that replay even
        // when the record is already retired.
        self.recover_job_transition(&job.job_id).await?;
        let mut lifecycle_exists = false;
        for prefix in crate::machine::JOB_PREFIXES {
            if self.read_job(prefix, &job.job_id).await?.is_some() {
                lifecycle_exists = true;
                break;
            }
        }
        if self.backend.exists(&transition_path(&job.job_id)).await? {
            return Err(StorageError::StorageConflict(format!(
                "durable prior admission exists for {}; refusing first-admission queue recreation",
                job.job_id
            )));
        }
        if lifecycle_exists {
            return Ok(false);
        }

        let blob_path = format!("queue/{}.json", job.job_id);
        // Index ordering rule (see `queue::listing` module docs): the marker
        // is written BEFORE the job blob is settled. Blob-then-marker leaves
        // an admitted job with no index entry if anything interrupts the
        // window — and since the index is the whole listing strategy for
        // `queue/`, that job is invisible to every scheduler while still
        // reporting `queued`. It used to self-heal only if the very same
        // caller retried admission under the same run id, which no other
        // participant can do on its behalf.
        //
        // Every queued job is indexed, not just the prioritized ones.
        // `priority_key` already sorts priority 0 correctly — it is the
        // largest inverted key, so those jobs land after all prioritized
        // work and FIFO among themselves — so this widens the index's
        // coverage without changing one byte of its name shape. The
        // listing walk can only be the ordered index if the index names
        // everything; while it named a subset, the unindexed remainder
        // needed a second, whole-prefix pass to be reachable at all.
        self.write_priority_marker(job).await?;
        let created = self
            .backend
            .upload_text_if_absent(&blob_path, &job.to_json())
            .await?;
        if created {
            let meta = Self::job_metadata(job);
            self.backend.set_metadata(&blob_path, &meta).await?;
        } else if !self.backend.exists(&blob_path).await? {
            // Lost the create to a generation that has since left `queue/`,
            // so the marker names nothing. Dropping it is an optimization,
            // not a correctness step: an orphan is skipped by the walk and
            // only costs scan budget.
            self.delete_priority_marker_for(job).await?;
        }
        Ok(created)
    }

    /// The planned job as it ended, when its durable transition record says
    /// it was admitted, moved into a terminal prefix and has since left every
    /// lifecycle prefix (the run reaper retires a settled job's documents).
    ///
    /// First admission refuses to recreate such a job, and that refusal is
    /// right: a terminal job id never re-enters the queue. But a replay of the
    /// same submission — a build resubmitted after its record lost the
    /// platform's job — then failed outright with "durable prior admission
    /// exists", and the build could never learn that its job had ended or
    /// queue a new attempt. The replay is answered with the job in the
    /// terminal state the record names, so its caller judges it as it judges
    /// any ended job. `None` when the record names no terminal destination.
    pub async fn ended_by_transition(&self, planned: &Job) -> Result<Option<Job>, StorageError> {
        let Some(state) = self.ended_state(&planned.job_id).await? else {
            return Ok(None);
        };
        let mut ended = planned.clone();
        ended.state = state;
        Ok(Some(ended))
    }

    /// The terminal prefix a job's durable transition record moved it into,
    /// the one witness of how it ended once the reaper has retired its
    /// documents; `None` when the record is gone or names no terminal prefix.
    pub async fn ended_state(&self, job_id: &str) -> Result<Option<String>, StorageError> {
        let Some(record) = self.backend.download_text(&transition_path(job_id)).await? else {
            return Ok(None);
        };
        let transition: crate::queue::storage::records::JobTransition =
            serde_json::from_str(&record)?;
        Ok(crate::queue::runs::TERMINAL_PREFIXES
            .contains(&transition.to_prefix.as_str())
            .then_some(transition.to_prefix))
    }

    /// Repair admission metadata only after a losing create has been read and
    /// validated against the durable planned job. A concurrent move turns this
    /// into a no-op and any marker written in that window is removed.
    pub async fn repair_queued_admission_metadata(
        &self,
        planned: &Job,
    ) -> Result<(), StorageError> {
        let path = format!("queue/{}.json", planned.job_id);
        let Some(versioned) = self.read_text_versioned(&path).await? else {
            return Ok(());
        };
        let current = Job::from_json(&versioned.content)?;
        if current.state != crate::models::job_state::QUEUED {
            return Ok(());
        }
        if crate::queue::submit::immutable_job_projection(&current)
            != crate::queue::submit::immutable_job_projection(planned)
        {
            return Err(StorageError::StorageConflict(format!(
                "{path} does not match validated durable admission"
            )));
        }
        match self
            .backend
            .set_metadata(&path, &Self::job_metadata(&current))
            .await
        {
            Ok(()) => {}
            Err(StorageError::NotFound(_)) => return Ok(()),
            Err(error) => return Err(error),
        }
        self.write_priority_marker(&current).await?;
        if !self.backend.exists(&path).await? {
            self.delete_priority_marker_for(&current).await?;
        }
        Ok(())
    }
}
