//! Durable cancellation: fence the request, then reclaim whatever the job
//! is recorded as holding.

use serde_json::{Map, Value};

use crate::machine::contract::encoding::{canonical_json, py_repr, utcnow};
use crate::machine::contract::jobs::{normalize_job, recorded_instance};
use crate::machine::{MachineError, MachineFacade};
use crate::models::job_state;
use crate::queue::StorageError;

impl MachineFacade {
    /// Durable, idempotent cancel (Python `cancel_job`): writes the
    /// `cancellations/<job_id>.json` marker first so the coordinator reaps
    /// even if this call dies mid-transition.
    ///
    /// Divergence from Python, which reads `job.instance_ref` and nothing
    /// else: the instance is resolved through [`recorded_instance`], so a
    /// VM whose reference only ever reached the provider lease is deleted
    /// too instead of billing forever. Every other step is unchanged.
    pub async fn cancel_job(&self, job_id: &str) -> Result<Value, MachineError> {
        let mut job = self.lookup_job(job_id).await?;
        if job_state::is_terminal(&job.state) {
            let mut out = Map::new();
            out.insert("job".into(), normalize_job(&job));
            return Ok(Value::Object(out));
        }

        let marker_path = format!("cancellations/{job_id}.json");
        let marker = canonical_json(&serde_json::json!({
            "job_id": job_id,
            "requested_at": utcnow(),
        }));
        self.store
            .create_text_if_absent(&marker_path, &marker)
            .await?;

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
                        Some(raced) => job = raced,
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
            if let Some(instance) = recorded_instance(&self.store, job_id)
                .await
                .map_err(|exc| MachineError::retryable("CANCEL_FAILED", exc.to_string()))?
            {
                if !instance.local {
                    let provider = crate::providers::get_provider(&instance.provider)
                        .map_err(|exc| MachineError::retryable("CANCEL_FAILED", exc.to_string()))?;
                    provider
                        .delete_instance(&instance.instance_ref)
                        .await
                        .map_err(|exc| {
                            MachineError::retryable(
                                "CANCEL_FAILED",
                                format!(
                                    "failed to delete instance {} recorded in {}: {exc}",
                                    instance.instance_ref, instance.source
                                ),
                            )
                        })?;
                }
            }
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
