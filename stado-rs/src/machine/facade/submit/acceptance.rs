//! Recording the submitted job in the reservation so a retry replays the
//! same answer instead of submitting twice.

use serde_json::{Map, Value};

use crate::machine::contract::encoding::{canonical_json, utcnow};
use crate::machine::contract::jobs::normalize_job;
use crate::machine::{MachineError, MachineFacade};
use crate::models::Job;
use crate::queue::StorageError;

use super::{ClaimedReservation, SubmitRequestContext};

impl MachineFacade {
    /// Finalize the reservation with the submitted job, and return the result
    /// every later retry of the same request will replay.
    pub(super) async fn accept_machine_request(
        &self,
        ctx: &SubmitRequestContext,
        reserved: &ClaimedReservation,
        job: &Job,
    ) -> Result<Value, MachineError> {
        let SubmitRequestContext {
            record_path,
            run_id,
            owner,
            ..
        } = ctx;
        let ClaimedReservation {
            source_uri,
            source_sha,
            ..
        } = reserved;
        let normalized = normalize_job(job);
        let mut result = Map::new();
        result.insert("job".into(), normalized.clone());
        if !source_uri.is_empty() {
            result.insert(
                "source_archive_uri".into(),
                Value::from(source_uri.as_str()),
            );
            result.insert("source_sha256".into(), Value::from(source_sha.as_str()));
        }
        for _ in 0..16 {
            let versioned = self
                .store
                .read_text_versioned(record_path)
                .await?
                .ok_or_else(|| {
                    MachineError::retryable(
                        "INTERNAL",
                        "submitted job reservation disappeared before acceptance",
                    )
                })?;
            let mut completed: Map<String, Value> =
                serde_json::from_str::<Value>(&versioned.content)?
                    .as_object()
                    .cloned()
                    .ok_or_else(|| {
                        MachineError::new("INTERNAL", "stored idempotency record is invalid")
                    })?;
            if let Some(stored) = completed.get("result").filter(|value| value.is_object()) {
                return Ok(stored.clone());
            }
            if completed.get("owner").and_then(Value::as_str) != Some(owner.as_str())
                || completed.get("state").and_then(Value::as_str) != Some("enqueuing")
                || completed.get("run_id").and_then(Value::as_str) != Some(run_id.as_str())
            {
                return Err(MachineError::retryable(
                    "INTERNAL",
                    format!(
                        "job {} was submitted but reservation ownership changed before acceptance",
                        job.job_id
                    ),
                ));
            }
            completed.insert("state".into(), Value::from("accepted"));
            completed.insert("job".into(), normalized.clone());
            completed.insert("result".into(), Value::Object(result.clone()));
            completed.insert("completed_at".into(), Value::from(utcnow()));
            completed.remove("owner");
            completed.remove("lease_expires_at");
            match self
                .store
                .compare_and_swap_text(
                    record_path,
                    &versioned.version,
                    &canonical_json(&Value::Object(completed)),
                )
                .await
            {
                Ok(_) => return Ok(Value::Object(result)),
                Err(StorageError::StorageConflict(_)) => continue,
                Err(error) => {
                    return Err(MachineError::retryable(
                        "INTERNAL",
                        format!(
                            "job {} was submitted but its idempotency record could not be finalized: {error}",
                            job.job_id
                        ),
                    ))
                }
            }
        }
        Err(MachineError::retryable(
            "INTERNAL",
            format!(
                "job {} was submitted but its idempotency record remained contended",
                job.job_id
            ),
        ))
    }
}
