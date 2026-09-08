//! Per-entry ownership: the fencing lease one submission takes on a run
//! entry, the claim that hands the planned job to the enqueue step, and the
//! CAS checkpoint that records acceptance.

use serde_json::{Map, Value};

use crate::models::Job;
use crate::queue::storage::JobStorage;
use crate::queue::submit::SubmitError;
use crate::queue::StorageError;

use super::validate_run_manifest;

pub(in crate::queue::submit) enum EntryClaim {
    Owned(Job),
    Accepted(Job),
    Terminal(Job),
}

fn lease_is_live(entry: &Map<String, Value>) -> bool {
    entry
        .get("lease_expires_at")
        .and_then(Value::as_str)
        .and_then(|value| chrono::DateTime::parse_from_rfc3339(value).ok())
        .is_some_and(|expires| expires > chrono::Utc::now())
}

/// The loop-invariant identity of one idempotent submission: the run manifest
/// every entry is claimed and checkpointed in, plus the immutable request that
/// names those entries and the ownership token fencing them.
///
/// `claim_entry` and `checkpoint_accepted` both need all of it and neither
/// varies any of it — only `index` and the created `job_id` differ between
/// calls — so it travels as one borrowed value instead of six parameters
/// re-threaded through every call site.
pub(in crate::queue::submit) struct SubmissionContext<'a> {
    pub(in crate::queue::submit) store: &'a JobStorage,
    pub(in crate::queue::submit) path: &'a str,
    pub(in crate::queue::submit) run_id: &'a str,
    pub(in crate::queue::submit) request: &'a Value,
    pub(in crate::queue::submit) request_digest: &'a str,
    pub(in crate::queue::submit) owner: &'a str,
}

pub(in crate::queue::submit) async fn claim_entry(
    ctx: &SubmissionContext<'_>,
    index: usize,
) -> Result<EntryClaim, SubmitError> {
    let SubmissionContext {
        store,
        path,
        run_id,
        request,
        request_digest,
        owner,
    } = *ctx;
    for _ in 0..16 {
        let versioned = store
            .read_text_versioned(path)
            .await?
            .ok_or_else(|| SubmitError::Validation(format!("run manifest {run_id} disappeared")))?;
        let mut manifest: Value = serde_json::from_str(&versioned.content)
            .map_err(|error| SubmitError::Validation(format!("invalid run manifest: {error}")))?;
        let validated = validate_run_manifest(&manifest, run_id, request, request_digest)?;
        let current = validated
            .get(index)
            .ok_or_else(|| SubmitError::Validation("run checkpoint entry is missing".into()))?;
        match current.state.as_str() {
            "accepted" => return Ok(EntryClaim::Accepted(current.planned_job.clone())),
            "terminal" | "reaped" => {
                return Ok(EntryClaim::Terminal(
                    current.outcome_job.clone().expect("validated outcome"),
                ))
            }
            _ => {}
        }
        let entry = manifest
            .get_mut("entries")
            .and_then(Value::as_array_mut)
            .and_then(|entries| entries.get_mut(index))
            .and_then(Value::as_object_mut)
            .ok_or_else(|| SubmitError::Validation("run checkpoint entry is missing".into()))?;
        let held_by = entry
            .get("owner")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if matches!(
            entry.get("state").and_then(Value::as_str),
            Some("claimed" | "enqueuing")
        ) && held_by != owner
            && lease_is_live(entry)
        {
            return Err(SubmitError::Validation(format!(
                "run {run_id} command {index} is being submitted by another owner"
            )));
        }
        entry.insert("state".into(), Value::from("claimed"));
        entry.insert("owner".into(), Value::from(owner));
        entry.insert(
            "lease_expires_at".into(),
            Value::from((chrono::Utc::now() + chrono::Duration::minutes(15)).to_rfc3339()),
        );
        let claimed_version = match store
            .compare_and_swap_text(
                path,
                &versioned.version,
                &serde_json::to_string_pretty(&manifest)
                    .map_err(|error| SubmitError::Validation(error.to_string()))?,
            )
            .await
        {
            Ok(version) => version,
            Err(StorageError::StorageConflict(_)) => continue,
            Err(error) => return Err(error.into()),
        };
        let entry = manifest
            .get_mut("entries")
            .and_then(Value::as_array_mut)
            .and_then(|entries| entries.get_mut(index))
            .and_then(Value::as_object_mut)
            .expect("validated entry");
        entry.insert("state".into(), Value::from("enqueuing"));
        match store
            .compare_and_swap_text(
                path,
                &claimed_version,
                &serde_json::to_string_pretty(&manifest)
                    .map_err(|error| SubmitError::Validation(error.to_string()))?,
            )
            .await
        {
            Ok(_) => return Ok(EntryClaim::Owned(current.planned_job.clone())),
            Err(StorageError::StorageConflict(_)) => continue,
            Err(error) => return Err(error.into()),
        }
    }
    Err(SubmitError::Validation(format!(
        "run manifest {run_id} remained contended while claiming command {index}"
    )))
}

// Every argument is a distinct coordinate of the checkpoint this writes: the
// store, the manifest path, the run, the request and its digest, which command
// index inside the run, the job that was accepted for it, and the owner that
// claimed it. Bundling them into a struct would only move the same eight names
// one level out, so the count is declared instead of hidden.
#[allow(clippy::too_many_arguments)]
pub(in crate::queue::submit) async fn checkpoint_accepted(
    ctx: &SubmissionContext<'_>,
    index: usize,
    job_id: &str,
) -> Result<(), SubmitError> {
    let SubmissionContext {
        store,
        path,
        run_id,
        request,
        request_digest,
        owner,
    } = *ctx;
    for _ in 0..16 {
        let versioned = store
            .read_text_versioned(path)
            .await?
            .ok_or_else(|| SubmitError::Validation(format!("run manifest {run_id} disappeared")))?;
        let mut manifest: Value = serde_json::from_str(&versioned.content)
            .map_err(|error| SubmitError::Validation(format!("invalid run manifest: {error}")))?;
        let validated = validate_run_manifest(&manifest, run_id, request, request_digest)?;
        let current = &validated[index];
        if current.state == "accepted" || matches!(current.state.as_str(), "terminal" | "reaped") {
            return Ok(());
        }
        let entry = manifest
            .get_mut("entries")
            .and_then(Value::as_array_mut)
            .and_then(|entries| entries.get_mut(index))
            .and_then(Value::as_object_mut)
            .ok_or_else(|| SubmitError::Validation("run checkpoint entry is missing".into()))?;
        if entry.get("job_id").and_then(Value::as_str) != Some(job_id)
            || entry.get("state").and_then(Value::as_str) != Some("enqueuing")
            || entry.get("owner").and_then(Value::as_str) != Some(owner)
        {
            return Err(SubmitError::Validation(
                "run checkpoint ownership changed before acceptance".into(),
            ));
        }
        entry.insert("state".into(), Value::from("accepted"));
        entry.insert(
            "accepted_at".into(),
            Value::from(chrono::Utc::now().to_rfc3339()),
        );
        entry.remove("owner");
        entry.remove("lease_expires_at");
        match store
            .compare_and_swap_text(
                path,
                &versioned.version,
                &serde_json::to_string_pretty(&manifest)
                    .map_err(|error| SubmitError::Validation(error.to_string()))?,
            )
            .await
        {
            Ok(_) => return Ok(()),
            Err(StorageError::StorageConflict(_)) => continue,
            Err(error) => return Err(error.into()),
        }
    }
    Err(SubmitError::Validation(format!(
        "run manifest {run_id} remained contended during checkpoint"
    )))
}
