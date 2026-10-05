//! Durable cancellation: fence the request, then reclaim whatever the job
//! is recorded as holding.

use serde_json::{Map, Value};

use crate::machine::contract::encoding::{py_repr, utcnow};
use crate::machine::contract::jobs::normalize_job;
use crate::machine::{
    capture_cancellation_allocation, fence_cancellation, request_provider_removal, MachineError,
    MachineFacade,
};
use crate::models::job_state;
use crate::queue::StorageError;

impl MachineFacade {
    /// Durable, idempotent cancel (Python `cancel_job`): writes the
    /// `cancellations/<job_id>.json` marker first so the coordinator reaps
    /// even if this call dies mid-transition.
    ///
    /// The recorded allocation is retained before its provider resource or
    /// terminal transition clears the reference. Status separately observes
    /// provider removal; accepting a delete request is not removal evidence.
    pub async fn cancel_job(&self, job_id: &str) -> Result<Value, MachineError> {
        let mut job = fence_cancellation(&self.store, job_id).await?;
        request_provider_removal(&self.store, &job).await?;
        if job_state::is_terminal(&job.state) {
            let mut out = Map::new();
            out.insert("job".into(), normalize_job(&job));
            return Ok(Value::Object(out));
        }

        if job.state == job_state::QUEUED {
            job.state = job_state::CANCELLED.into();
            job.completed_at = Some(utcnow());
            job.error = Some("cancelled".into());
            match self.store.move_job(&job, "queue", "cancelled").await {
                Ok(()) => {
                    let mut out = Map::new();
                    out.insert("job".into(), normalize_job(&job));
                    return Ok(Value::Object(out));
                }
                Err(StorageError::StorageConflict(_)) => {
                    match self.store.read_job("running", job_id).await? {
                        Some(raced) => {
                            job = raced;
                            capture_cancellation_allocation(&self.store, &job).await?;
                            request_provider_removal(&self.store, &job).await?;
                        }
                        None => {
                            return Err(MachineError::retryable(
                                "CANCEL_FAILED",
                                "job moved while cancellation was being fenced",
                            ))
                        }
                    }
                }
                Err(error) => return Err(error.into()),
            }
        }

        if job.state == job_state::RUNNING {
            job.state = job_state::CANCELLED.into();
            job.completed_at = Some(utcnow());
            job.error = Some("cancelled".into());
            job.instance_ref = None;
            self.store
                .move_job(&job, "running", "cancelled")
                .await
                .map_err(|error| {
                    MachineError::retryable(
                        "CANCEL_FAILED",
                        format!("job moved while cancellation was being fenced: {error}"),
                    )
                })?;
            let mut out = Map::new();
            out.insert("job".into(), normalize_job(&job));
            return Ok(Value::Object(out));
        }

        Err(MachineError::retryable(
            "CANCEL_FAILED",
            format!("job {} is in an unsupported state", py_repr(job_id)),
        ))
    }
}
