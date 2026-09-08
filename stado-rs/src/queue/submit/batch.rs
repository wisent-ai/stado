//! The submission itself: persist the immutable plan, then create each stable
//! command job exactly once, checkpoint its acceptance and return the receipt
//! the caller submitted for.

use serde_json::Value;

use crate::config;
use crate::models::{activation_extraction_must_share_gpu, Job};
use crate::queue::runs::RUN_PREFIX;
use crate::queue::storage::JobStorage;

use super::{
    build_planned_job, checkpoint_accepted, claim_entry, digest_value, hostname, job_id_from_key,
    migrate_v2_run_manifest, resolve_hardware, submission_input_digest, submission_job_key,
    submission_request, submission_source_digest, submitter, validate_recovered_job,
    validate_run_id, validate_run_manifest, validate_submission, EntryClaim,
    ResolvedHardwareProjection, SubmissionContext, SubmissionProvenance, SubmitError,
    SubmitOptions,
};

async fn find_job(store: &JobStorage, job_id: &str) -> Result<Option<Job>, SubmitError> {
    for prefix in [
        "cancelled",
        "failed",
        "uploaded",
        "completed",
        "running",
        "queue",
    ] {
        if let Some(job) = store.read_job(prefix, job_id).await? {
            return Ok(Some(job));
        }
    }
    Ok(None)
}

/// Persist a complete immutable plan before any queue write, then create each
/// stable command job exactly once and CAS-checkpoint acceptance in order.
pub async fn submit_batch(
    commands: &[String],
    options: &SubmitOptions,
) -> Result<Vec<Job>, SubmitError> {
    if commands.is_empty() {
        return Err(SubmitError::Validation(
            "at least one command is required".into(),
        ));
    }
    let options = options.clone();
    validate_run_id(&options.run_id)?;
    for command in commands {
        validate_submission(command, &options)?;
    }
    let run_id = options.run_id.clone();
    let bucket = if options.bucket.is_empty() {
        config::bucket()
    } else {
        options.bucket.as_str()
    };
    let store = JobStorage::with_bucket(bucket).await?;
    let path = format!("{RUN_PREFIX}/{run_id}.json");
    let existing_raw = if store.download_text(&path).await?.is_some() {
        Some(
            serde_json::to_string(&migrate_v2_run_manifest(&store, &run_id).await?)
                .map_err(|error| SubmitError::Validation(error.to_string()))?,
        )
    } else {
        None
    };
    let resolved_hardware: Vec<ResolvedHardwareProjection> = if let Some(raw) =
        existing_raw.as_ref()
    {
        let existing: Value = serde_json::from_str(raw)
            .map_err(|error| SubmitError::Validation(format!("invalid run manifest: {error}")))?;
        if existing.get("schema").and_then(Value::as_str) == Some("stado.run-submission.v3") {
            let stored_request = existing
                .get("request")
                .ok_or_else(|| SubmitError::Validation("stored run request is missing".into()))?;
            let expected_options = serde_json::to_value(&options).map_err(|error| {
                SubmitError::Validation(format!("serialize submission options: {error}"))
            })?;
            if stored_request.get("schema").and_then(Value::as_str)
                != Some("stado.submission-request.v3")
                || stored_request.get("commands") != Some(&serde_json::json!(commands))
                || stored_request.get("options") != Some(&expected_options)
                || stored_request
                    .get("effective_bucket")
                    .and_then(Value::as_str)
                    != Some(bucket)
            {
                return Err(SubmitError::Validation(format!(
                    "run id {run_id} already belongs to a different submission request"
                )));
            }
            serde_json::from_value(
                stored_request
                    .get("resolved_hardware")
                    .cloned()
                    .ok_or_else(|| {
                        SubmitError::Validation(
                            "stored run request has no resolved hardware plan".into(),
                        )
                    })?,
            )
            .map_err(|error| {
                SubmitError::Validation(format!(
                    "stored run request has invalid resolved hardware: {error}"
                ))
            })?
        } else {
            return Err(SubmitError::Validation(format!(
                "run id {run_id} has an unsupported manifest schema"
            )));
        }
    } else {
        let mut resolved = Vec::with_capacity(commands.len());
        for command in commands {
            resolved.push(resolve_hardware(command, &options).await?);
        }
        resolved
    };
    if resolved_hardware.len() != commands.len() {
        return Err(SubmitError::Validation(format!(
            "run id {run_id} has an invalid resolved hardware plan"
        )));
    }
    let request = submission_request(commands, &options, &resolved_hardware)?;
    let request_digest = digest_value(&request);

    let manifest = match existing_raw {
        Some(raw) => serde_json::from_str(&raw)
            .map_err(|error| SubmitError::Validation(format!("invalid run manifest: {error}")))?,
        None => {
            let provenance = SubmissionProvenance {
                created_at: chrono::Utc::now().to_rfc3339(),
                submitter_app: std::env::var("WC_SUBMITTER_APP")
                    .ok()
                    .filter(|value| !value.is_empty())
                    .unwrap_or_else(|| "manual".into()),
                submitted_by: submitter(),
                submitted_from: hostname(),
            };
            let mut entries = Vec::with_capacity(commands.len());
            for (index, command) in commands.iter().enumerate() {
                let key = submission_job_key(&request_digest, index, command);
                let job_id = job_id_from_key(&key);
                let mut effective = options.clone();
                effective.exclusive =
                    effective.exclusive && !activation_extraction_must_share_gpu(command);
                let mut job = build_planned_job(
                    command,
                    &effective,
                    &job_id,
                    &resolved_hardware[index],
                    &provenance,
                );
                job.submission_request_digest = request_digest.clone();
                job.submission_command_index = Some(index);
                entries.push(serde_json::json!({
                    "command_index": index,
                    "command": command,
                    "job_key": key,
                    "job_id": job_id,
                    "state": "planned",
                    "planned_job": job,
                }));
            }
            let explicit_name = std::env::var("WC_RUN_NAME").unwrap_or_default();
            let candidate = serde_json::json!({
                "schema": "stado.run-submission.v3",
                "run_id": run_id,
                "name": if explicit_name.is_empty() {
                    crate::queue::runs::derive_run_name(commands)
                } else {
                    explicit_name
                },
                "request_digest": request_digest,
                "source_digest": submission_source_digest(&options),
                "input_digest": submission_input_digest(commands, &options),
                "request": request,
                "created_at": provenance.created_at,
                "submitter_app": provenance.submitter_app,
                "submitted_by": provenance.submitted_by,
                "submitted_from": provenance.submitted_from,
                "entries": entries,
            });
            let created = store
                .create_text_if_absent(
                    &path,
                    &serde_json::to_string_pretty(&candidate)
                        .map_err(|error| SubmitError::Validation(error.to_string()))?,
                )
                .await?;
            if created {
                candidate
            } else {
                migrate_v2_run_manifest(&store, &run_id).await?
            }
        }
    };
    let planned = validate_run_manifest(&manifest, &run_id, &request, &request_digest)?;
    // Submission ownership is stable for the immutable request. If a caller
    // drops this future after a machine-request lease renewal failure, the next
    // idempotent replay can resume immediately instead of waiting fifteen
    // minutes for a random, now-ownerless token to expire. Concurrent replays
    // share only this exact request digest and all side effects remain
    // create-if-absent/CAS fenced.
    let owner = format!("submission:{request_digest}");
    let ctx = SubmissionContext {
        store: &store,
        path: &path,
        run_id: &run_id,
        request: &request,
        request_digest: &request_digest,
        owner: &owner,
    };
    let mut accepted = Vec::with_capacity(planned.len());
    for index in 0..planned.len() {
        match claim_entry(&ctx, index).await? {
            EntryClaim::Terminal(job) => accepted.push(job),
            EntryClaim::Accepted(planned_job) => {
                let existing = find_job(&store, &planned_job.job_id)
                    .await?
                    .ok_or_else(|| {
                        SubmitError::Validation(format!(
                            "accepted stable job {} is absent; refusing to recreate it",
                            planned_job.job_id
                        ))
                    })?;
                validate_recovered_job(&existing, &planned_job, index)?;
                store.repair_queued_admission_metadata(&planned_job).await?;
                accepted.push(existing);
            }
            EntryClaim::Owned(planned_job) => {
                let job = if let Some(existing) = find_job(&store, &planned_job.job_id).await? {
                    validate_recovered_job(&existing, &planned_job, index)?;
                    store.repair_queued_admission_metadata(&planned_job).await?;
                    existing
                } else if store.create_queued_job_if_absent(&planned_job).await? {
                    planned_job.clone()
                } else {
                    let existing =
                        find_job(&store, &planned_job.job_id)
                            .await?
                            .ok_or_else(|| {
                                SubmitError::Validation(format!(
                                    "stable job {} was concurrently created but is unreadable",
                                    planned_job.job_id
                                ))
                            })?;
                    validate_recovered_job(&existing, &planned_job, index)?;
                    store.repair_queued_admission_metadata(&planned_job).await?;
                    existing
                };
                checkpoint_accepted(&ctx, index, &job.job_id).await?;
                accepted.push(job);
            }
        }
    }
    Ok(accepted)
}
