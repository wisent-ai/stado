//! Claiming and leasing: settling queued cancellation requests, taking a
//! queued job into `running/` with a lease, and renewing that lease.

use chrono::Utc;

use crate::config;
use crate::models::Job;
use crate::queue::storage::JobStorage;
use crate::queue::StorageError;

impl JobStorage {
    /// Finish durable cancellation requests for jobs that remain in `queue/`.
    ///
    /// The cancellation marker is written before the lifecycle move, so a
    /// caller that exits between those writes leaves work for the next
    /// coordinator tick. Claiming deliberately refuses a marked job, but a
    /// refusal alone strands its queued projection forever. Enumerating the
    /// active queue first keeps this work bounded by pending jobs: historical
    /// markers for terminal or absent jobs are neither downloaded nor parsed.
    ///
    /// Each relevant job's prepared transition is recovered before its fresh,
    /// versioned queue body is read. The normal CAS lifecycle transition then
    /// creates the durable `cancelled/` projection and retires `queue/`; the
    /// request marker and independent `runs/` provenance remain untouched.
    pub async fn settle_queued_cancellations(&self) -> Result<usize, StorageError> {
        let mut settled = 0;
        for job_id in self.list_job_ids("queue").await? {
            self.recover_job_transition(&job_id).await?;
            let queue_path = format!("queue/{job_id}.json");
            let Some(versioned) = self.read_text_versioned(&queue_path).await? else {
                continue;
            };
            let mut job = Job::from_json(&versioned.content)?;
            if job.job_id != job_id || job.state != crate::models::job_state::QUEUED {
                continue;
            }

            let marker_path = format!("cancellations/{job_id}.json");
            let Some(raw_request) = self.backend.download_text(&marker_path).await? else {
                continue;
            };
            let request: serde_json::Value = serde_json::from_str(&raw_request)?;
            if request.get("job_id").and_then(serde_json::Value::as_str) != Some(job_id.as_str()) {
                return Err(StorageError::Other(format!(
                    "{marker_path} does not name its own job id"
                )));
            }
            let requested_at = request
                .get("requested_at")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| StorageError::Other(format!("{marker_path} has no requested_at")))?;
            chrono::DateTime::parse_from_rfc3339(requested_at).map_err(|error| {
                StorageError::Other(format!("{marker_path} has invalid requested_at: {error}"))
            })?;

            job.state = crate::models::job_state::CANCELLED.to_string();
            job.completed_at = Some(requested_at.to_string());
            job.error = Some("cancelled".to_string());
            if self
                .transition_job_if_version(&job, "queue", "cancelled", Some(&versioned.version))
                .await?
            {
                settled += 1;
            }
        }
        Ok(settled)
    }

    /// Claim through a durable transition record. The running job is derived
    /// from the fresh versioned queue body; stale caller priority, assignment,
    /// or provider placement cannot overwrite a concurrent queued rewrite.
    ///
    /// The claimed running document carries a worker lease from the first
    /// instant it exists, so a claim that dies before its first heartbeat is
    /// still reaped on a stated expiry rather than on a guess about
    /// `started_at`.
    pub async fn claim_queued_job(&self, job: &Job) -> Result<bool, StorageError> {
        self.recover_job_transition(&job.job_id).await?;
        let queue_path = format!("queue/{}.json", job.job_id);
        let Some(versioned) = self.read_text_versioned(&queue_path).await? else {
            return Ok(false);
        };
        let current = Job::from_json(&versioned.content)?;
        if current.state != crate::models::job_state::QUEUED
            || current.assigned_to != job.assigned_to
            || current.provider != job.provider
            || current.pin_to_provider != job.pin_to_provider
            || crate::queue::submit::immutable_job_projection(&current)
                != crate::queue::submit::immutable_job_projection(job)
        {
            return Ok(false);
        }
        let cancellation = format!("cancellations/{}.json", job.job_id);
        let cancelled = format!("cancelled/{}.json", job.job_id);
        if self.backend.exists(&cancellation).await? || self.backend.exists(&cancelled).await? {
            return Ok(false);
        }
        let mut claimed = job.clone();
        claimed.lease_expires_at = Some(Self::lease_deadline());
        let moved = self
            .transition_job_if_version(&claimed, "queue", "running", Some(&versioned.version))
            .await?;
        if !moved {
            return Ok(false);
        }
        if self.backend.exists(&cancellation).await? || self.backend.exists(&cancelled).await? {
            return Ok(false);
        }
        Ok(true)
    }

    /// One worker-lease deadline from now, in the window the fleet already
    /// calls a dead heartbeat.
    fn lease_deadline() -> String {
        (Utc::now() + chrono::Duration::minutes(config::HEARTBEAT_STALE_MINUTES)).to_rfc3339()
    }

    /// Renew the running job's own lease by compare-and-swap on the running
    /// document.
    ///
    /// This is the fence, and it only works because it writes the SAME object
    /// the reaper pins: a renewal that lands while a reaper is mid-reap
    /// changes that object's version, so the reaper's version-pinned move
    /// fails and the live execution keeps its slot. A pulse written beside the
    /// job cannot do that, however recently it was read.
    ///
    /// `false` when the job is no longer a live running document (moved,
    /// deleted, or fenced mid-transition) — the caller has lost the job, not
    /// the write.
    pub async fn renew_running_lease(&self, job_id: &str) -> Result<bool, StorageError> {
        let path = format!("running/{job_id}.json");
        for _ in 0..3 {
            let Some(versioned) = self.read_text_versioned(&path).await? else {
                return Ok(false);
            };
            let mut job = Job::from_json(&versioned.content)?;
            if job.state != crate::models::job_state::RUNNING {
                return Ok(false);
            }
            job.lease_expires_at = Some(Self::lease_deadline());
            match self
                .compare_and_swap_text(&path, &versioned.version, &job.to_json())
                .await
            {
                Ok(_) => return Ok(true),
                Err(StorageError::StorageConflict(_)) => continue,
                Err(StorageError::NotFound(_)) => return Ok(false),
                Err(error) => return Err(error),
            }
        }
        Err(StorageError::StorageConflict(format!(
            "running/{job_id}.json remained contended during lease renewal"
        )))
    }
}
