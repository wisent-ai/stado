//! In-place upgrade of a durable v2 run manifest to the v3 request plan.

use serde_json::Value;

use crate::models::Job;
use crate::queue::runs::RUN_PREFIX;
use crate::queue::storage::JobStorage;
use crate::queue::submit::{
    digest_value, job_id_from_key, submission_input_digest, submission_job_key,
    submission_source_digest, validate_run_id, ResolvedHardwareProjection, SubmitError,
    SubmitOptions,
};
use crate::queue::StorageError;

use super::validate_run_manifest;

/// Idempotently upgrade a durable v2 run in place. The v3 request digest
/// includes the hardware plan, but admitted v2 job identities are immutable:
/// entries retain their original job IDs and submission digest while their
/// v3 job keys are remapped to the upgraded request.
pub(crate) async fn migrate_v2_run_manifest(
    store: &JobStorage,
    run_id: &str,
) -> Result<Value, SubmitError> {
    validate_run_id(run_id)?;
    let path = format!("{RUN_PREFIX}/{run_id}.json");
    for _ in 0..16 {
        let versioned = store
            .read_text_versioned(&path)
            .await?
            .ok_or_else(|| StorageError::NotFound(path.clone()))?;
        let mut manifest: Value = serde_json::from_str(&versioned.content)
            .map_err(|error| SubmitError::Validation(format!("invalid run manifest: {error}")))?;
        let schema = manifest.get("schema").and_then(Value::as_str);
        if schema != Some("stado.run-submission.v2") {
            return Ok(manifest);
        }
        if manifest.get("run_id").and_then(Value::as_str) != Some(run_id) {
            return Err(SubmitError::Validation(format!(
                "v2 run manifest does not match run id {run_id}"
            )));
        }
        let old_request = manifest
            .get("request")
            .cloned()
            .ok_or_else(|| SubmitError::Validation("v2 run manifest has no request".into()))?;
        if old_request.get("schema").and_then(Value::as_str) != Some("stado.submission-request.v2")
        {
            return Err(SubmitError::Validation(format!(
                "run id {run_id} has an invalid v2 request schema"
            )));
        }
        let old_digest = digest_value(&old_request);
        if manifest.get("request_digest").and_then(Value::as_str) != Some(old_digest.as_str()) {
            return Err(SubmitError::Validation(format!(
                "run id {run_id} has a corrupt v2 request digest"
            )));
        }
        let commands: Vec<String> = old_request
            .get("commands")
            .and_then(Value::as_array)
            .ok_or_else(|| SubmitError::Validation("v2 submission commands are missing".into()))?
            .iter()
            .map(|command| {
                command.as_str().map(str::to_string).ok_or_else(|| {
                    SubmitError::Validation("v2 submission command is not a string".into())
                })
            })
            .collect::<Result<_, _>>()?;
        let options: SubmitOptions =
            serde_json::from_value(old_request.get("options").cloned().ok_or_else(|| {
                SubmitError::Validation("v2 submission options are missing".into())
            })?)
            .map_err(|error| {
                SubmitError::Validation(format!("invalid v2 submission options: {error}"))
            })?;
        let entries = manifest
            .get("entries")
            .and_then(Value::as_array)
            .ok_or_else(|| SubmitError::Validation("v2 run entries are missing".into()))?;
        if entries.len() != commands.len() {
            return Err(SubmitError::Validation(format!(
                "run id {run_id} has an incomplete v2 command plan"
            )));
        }
        let mut resolved_hardware = Vec::with_capacity(entries.len());
        for (index, entry) in entries.iter().enumerate() {
            let planned: Job =
                serde_json::from_value(entry.get("planned_job").cloned().ok_or_else(|| {
                    SubmitError::Validation("v2 run entry has no planned job".into())
                })?)
                .map_err(|error| {
                    SubmitError::Validation(format!("invalid v2 planned job: {error}"))
                })?;
            let old_key = submission_job_key(&old_digest, index, &commands[index]);
            let old_job_id = job_id_from_key(&old_key);
            if entry.get("command_index").and_then(Value::as_u64) != Some(index as u64)
                || entry.get("command").and_then(Value::as_str) != Some(commands[index].as_str())
                || entry.get("job_key").and_then(Value::as_str) != Some(old_key.as_str())
                || entry.get("job_id").and_then(Value::as_str) != Some(old_job_id.as_str())
                || planned.job_id != old_job_id
                || planned.run_id != run_id
                || planned.submission_request_digest != old_digest
                || planned.submission_command_index != Some(index)
            {
                return Err(SubmitError::Validation(format!(
                    "run id {run_id} has a corrupt v2 entry at index {index}"
                )));
            }
            resolved_hardware.push(ResolvedHardwareProjection {
                gpu_mem_gb: planned.gpu_mem_gb,
                gpu_type: planned.gpu_type,
                machine_type: planned.machine_type,
            });
        }
        let normalized_options = serde_json::to_value(&options).map_err(|error| {
            SubmitError::Validation(format!("serialize migrated submission options: {error}"))
        })?;
        let effective_bucket = old_request
            .get("effective_bucket")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                SubmitError::Validation("v2 request effective bucket is missing".into())
            })?;
        let request = serde_json::json!({
            "schema": "stado.submission-request.v3",
            "commands": commands,
            "effective_bucket": effective_bucket,
            "options": normalized_options,
            "resolved_hardware": resolved_hardware,
        });
        let request_digest = digest_value(&request);
        let object = manifest
            .as_object_mut()
            .ok_or_else(|| SubmitError::Validation("v2 run manifest is not an object".into()))?;
        object.insert("schema".into(), Value::from("stado.run-submission.v3"));
        object.insert("request".into(), request.clone());
        object.insert(
            "request_digest".into(),
            Value::from(request_digest.as_str()),
        );
        object.insert(
            "migrated_from_v2_request_digest".into(),
            Value::from(old_digest.as_str()),
        );
        object.insert(
            "source_digest".into(),
            Value::from(submission_source_digest(&options)),
        );
        object.insert(
            "input_digest".into(),
            Value::from(submission_input_digest(&commands, &options)),
        );
        for obsolete in ["n_jobs", "job_ids", "commands"] {
            object.remove(obsolete);
        }
        let migrated_entries = object
            .get_mut("entries")
            .and_then(Value::as_array_mut)
            .expect("validated entries");
        for (index, entry) in migrated_entries.iter_mut().enumerate() {
            let entry = entry
                .as_object_mut()
                .ok_or_else(|| SubmitError::Validation("v2 run entry is not an object".into()))?;
            entry.insert(
                "job_key".into(),
                Value::from(submission_job_key(&request_digest, index, &commands[index])),
            );
        }
        validate_run_manifest(&manifest, run_id, &request, &request_digest)?;
        let body = serde_json::to_string_pretty(&manifest)
            .map_err(|error| SubmitError::Validation(error.to_string()))?;
        match store
            .compare_and_swap_text(&path, &versioned.version, &body)
            .await
        {
            Ok(_) => return Ok(manifest),
            Err(StorageError::StorageConflict(_) | StorageError::NotFound(_)) => continue,
            Err(error) => return Err(error.into()),
        }
    }
    Err(SubmitError::Validation(format!(
        "run manifest {run_id} remained contended during v2 migration"
    )))
}
