//! Rewrites and moves of an existing job document: the shared CAS rewrite of
//! a current queued generation, the operator-facing updates built on it, the
//! delete, and the two lifecycle moves.

use crate::models::Job;
use crate::queue::storage::JobStorage;
use crate::queue::{listing, StorageError};

impl JobStorage {
    async fn rewrite_queued_job<F>(
        &self,
        job_id: &str,
        mutate: F,
    ) -> Result<Option<Job>, StorageError>
    where
        F: Fn(&mut Job),
    {
        let path = format!("queue/{job_id}.json");
        // A transition still pending on this job is finished before the
        // document is rewritten, so the rewrite never lands under a fence
        // another writer is about to retire — and an operator's own
        // `job set-priority` becomes a way to finish a transition the agent
        // cannot, which is what cleared `charless-mac-mini` on 2026-09-03.
        self.recover_job_transition(job_id).await?;
        for _ in 0..3 {
            let Some(versioned) = self.read_text_versioned(&path).await? else {
                return Ok(None);
            };
            let mut job = Job::from_json(&versioned.content)?;
            if job.state != crate::models::job_state::QUEUED {
                return Ok(None);
            }
            mutate(&mut job);
            match self
                .compare_and_swap_text(&path, &versioned.version, &job.to_json())
                .await
            {
                Ok(_) => {}
                Err(StorageError::StorageConflict(_)) => continue,
                Err(error) => return Err(error),
            }
            if !self.backend.exists(&path).await? {
                return Ok(None);
            }
            if let Err(error) = self
                .backend
                .set_metadata(&path, &Self::job_metadata(&job))
                .await
            {
                if matches!(&error, StorageError::NotFound(_)) {
                    return Ok(None);
                }
                return Err(error);
            }
            if !self.backend.exists(&path).await? {
                return Ok(None);
            }
            return Ok(Some(job));
        }
        Err(StorageError::StorageConflict(format!(
            "queue/{job_id}.json remained contended during rewrite"
        )))
    }

    /// CAS-update one current queued generation's priority and marker.
    pub async fn update_queued_priority(
        &self,
        job_id: &str,
        new_priority: i64,
    ) -> Result<Option<Job>, StorageError> {
        if !(0..=99_999_999).contains(&new_priority) {
            return Err(StorageError::Other(
                "job priority must be between 0 and 99999999".into(),
            ));
        }
        let updated = self
            .rewrite_queued_job(job_id, |job| job.priority = new_priority)
            .await?;
        // Per the index ordering rule (see `queue::listing` module docs), the
        // new key is written BEFORE the superseded one is dropped. A priority
        // change re-keys the marker, and clean-then-write leaves the job with
        // NO marker if anything interrupts between the two steps — a
        // transient 5xx, or an operator's Ctrl-C on `job priority` — which
        // strands a queued job outside the index permanently. Write-then-
        // clean fails into a harmless duplicate under the old key instead.
        // `keep` stops the cleanup scan, which matches on job_id, from
        // deleting the marker just written.
        if let Some(job) = &updated {
            self.write_priority_marker(job).await?;
            let current = listing::marker_path(job);
            self.repair_priority_markers(job_id, Some(&current)).await?;
            if !self.backend.exists(&format!("queue/{job_id}.json")).await? {
                self.delete_priority_marker_for(job).await?;
                return Ok(None);
            }
        } else {
            // No current queued generation to index; anything still bearing
            // this id is an orphan.
            self.repair_priority_markers(job_id, None).await?;
        }
        Ok(updated)
    }

    /// CAS-update the measured queue sizing without recreating moved work.
    pub async fn update_queued_gpu_mem(
        &self,
        job_id: &str,
        gpu_mem_gb: i64,
    ) -> Result<Option<Job>, StorageError> {
        self.rewrite_queued_job(job_id, |job| job.gpu_mem_gb = gpu_mem_gb)
            .await
    }

    /// CAS-update the makespan assignment without recreating moved work.
    /// CAS-update one current queued generation's placement: the provider it
    /// is pinned to and the capacity it is assigned to. Goes through the same
    /// rewrite as priority and assignment so a transition still pending on
    /// the job is recovered before the document is touched; a placement
    /// written under a pending transition is what left `charless-mac-mini`
    /// refusing to claim on 2026-09-03.
    pub async fn update_queued_placement(
        &self,
        job_id: &str,
        provider: &str,
        assigned_to: Option<&str>,
    ) -> Result<Option<Job>, StorageError> {
        self.rewrite_queued_job(job_id, |job| {
            job.provider = provider.to_string();
            job.pin_to_provider = true;
            match assigned_to {
                Some(target) => job.assigned_to = target.to_string(),
                None => job.assigned_to.clear(),
            }
        })
        .await
    }

    pub async fn update_queued_assignment(
        &self,
        job_id: &str,
        assigned_to: &str,
    ) -> Result<Option<Job>, StorageError> {
        self.rewrite_queued_job(job_id, |job| job.assigned_to = assigned_to.to_string())
            .await
    }

    /// Delete the job blob; also drops the priority marker in `queue/`.
    ///
    /// The job is read before it is deleted so the marker can be removed by
    /// its exact name. A job already gone leaves a marker whose key cannot be
    /// computed, which is the orphan case the index walk repairs.
    pub async fn delete_job(&self, prefix: &str, job_id: &str) -> Result<(), StorageError> {
        let indexed = if prefix == "queue" {
            self.read_job(prefix, job_id).await?
        } else {
            None
        };
        self.delete_blob(&format!("{prefix}/{job_id}.json")).await?;
        if prefix == "queue" {
            match &indexed {
                Some(job) => self.delete_priority_marker_for(job).await?,
                None => self.repair_priority_markers(job_id, None).await?,
            }
        }
        Ok(())
    }

    /// Move through the recoverable transition record. No destination is
    /// created until the exact source generation has been fenced.
    pub async fn move_job(
        &self,
        job: &Job,
        from_prefix: &str,
        to_prefix: &str,
    ) -> Result<(), StorageError> {
        if self
            .transition_job_if_version(job, from_prefix, to_prefix, None)
            .await?
        {
            Ok(())
        } else {
            Err(StorageError::StorageConflict(format!(
                "{from_prefix}/{}.json changed before transition to {to_prefix}",
                job.job_id
            )))
        }
    }

    /// Version-pinned lifecycle move for decisions (lease expiry, liveness)
    /// made from a specific source read.
    pub async fn move_job_if_version(
        &self,
        job: &Job,
        from_prefix: &str,
        to_prefix: &str,
        expected_version: &str,
    ) -> Result<bool, StorageError> {
        self.transition_job_if_version(job, from_prefix, to_prefix, Some(expected_version))
            .await
    }
}
