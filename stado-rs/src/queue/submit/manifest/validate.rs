//! Proof that a stored run manifest is exactly the plan its immutable request
//! derives, entry by entry, including retained terminal outcomes.

use chrono::{DateTime, Utc};
use serde_json::Value;

use crate::models::{activation_extraction_must_share_gpu, Job};
use crate::queue::submit::{
    build_planned_job, job_id_from_key, submission_input_digest, submission_job_key,
    submission_source_digest, ResolvedHardwareProjection, SubmissionProvenance, SubmitError,
    SubmitOptions,
};

use super::ManifestEntry;

pub(in crate::queue::submit) fn validate_run_manifest(
    manifest: &Value,
    run_id: &str,
    request: &Value,
    request_digest: &str,
) -> Result<Vec<ManifestEntry>, SubmitError> {
    if manifest.get("schema").and_then(Value::as_str) != Some("stado.run-submission.v3")
        || manifest.get("run_id").and_then(Value::as_str) != Some(run_id)
        || manifest.get("request_digest").and_then(Value::as_str) != Some(request_digest)
        || manifest.get("request") != Some(request)
    {
        return Err(SubmitError::Validation(format!(
            "run id {run_id} already belongs to a different or legacy submission request"
        )));
    }
    if request.get("schema").and_then(Value::as_str) != Some("stado.submission-request.v3") {
        return Err(SubmitError::Validation(format!(
            "run id {run_id} requires explicit request-plan migration"
        )));
    }
    for obsolete in ["n_jobs", "job_ids", "commands"] {
        if manifest.get(obsolete).is_some() {
            return Err(SubmitError::Validation(format!(
                "run id {run_id} retained obsolete parallel manifest field {obsolete}"
            )));
        }
    }
    let commands: Vec<String> = request
        .get("commands")
        .and_then(Value::as_array)
        .ok_or_else(|| SubmitError::Validation("submission request commands are missing".into()))?
        .iter()
        .map(|command| {
            command
                .as_str()
                .map(str::to_string)
                .ok_or_else(|| SubmitError::Validation("submission command is not a string".into()))
        })
        .collect::<Result<_, _>>()?;
    let options: SubmitOptions =
        serde_json::from_value(request.get("options").cloned().ok_or_else(|| {
            SubmitError::Validation("submission request options are missing".into())
        })?)
        .map_err(|error| SubmitError::Validation(format!("invalid submission options: {error}")))?;
    let resolved_hardware: Vec<ResolvedHardwareProjection> =
        serde_json::from_value(request.get("resolved_hardware").cloned().ok_or_else(|| {
            SubmitError::Validation("submission request hardware plan is missing".into())
        })?)
        .map_err(|error| {
            SubmitError::Validation(format!("invalid submission hardware plan: {error}"))
        })?;
    if resolved_hardware.len() != commands.len() {
        return Err(SubmitError::Validation(format!(
            "run id {run_id} has an incomplete submission hardware plan"
        )));
    }
    let required_manifest_string = |field: &str| {
        manifest
            .get(field)
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| {
                SubmitError::Validation(format!("run manifest {run_id} is missing {field}"))
            })
    };
    let provenance = SubmissionProvenance {
        created_at: required_manifest_string("created_at")?,
        submitted_by: required_manifest_string("submitted_by")?,
        submitted_from: required_manifest_string("submitted_from")?,
        submitter_app: required_manifest_string("submitter_app")?,
    };
    let manifest_created_at = DateTime::parse_from_rfc3339(&provenance.created_at)
        .map_err(|_| {
            SubmitError::Validation(format!("run manifest {run_id} has invalid created_at"))
        })?
        .with_timezone(&Utc);
    if manifest.get("source_digest").and_then(Value::as_str)
        != Some(submission_source_digest(&options).as_str())
        || manifest.get("input_digest").and_then(Value::as_str)
            != Some(submission_input_digest(&commands, &options).as_str())
    {
        return Err(SubmitError::Validation(format!(
            "run id {run_id} has corrupt source or input digests"
        )));
    }
    let entries = manifest
        .get("entries")
        .and_then(Value::as_array)
        .ok_or_else(|| SubmitError::Validation("run manifest entries are missing".into()))?;
    if entries.len() != commands.len() {
        return Err(SubmitError::Validation(format!(
            "run id {run_id} has {} entries for {} commands",
            entries.len(),
            commands.len()
        )));
    }
    let legacy_request_digest = manifest
        .get("migrated_from_v2_request_digest")
        .and_then(Value::as_str);
    if legacy_request_digest.is_some_and(|digest| {
        digest.len() != 64
            || !digest
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    }) {
        return Err(SubmitError::Validation(format!(
            "run id {run_id} has an invalid v2 identity digest"
        )));
    }
    let mut validated = Vec::with_capacity(entries.len());
    for (index, entry) in entries.iter().enumerate() {
        let command = &commands[index];
        let key = submission_job_key(request_digest, index, command);
        let identity_digest = legacy_request_digest.unwrap_or(request_digest);
        let identity_key = submission_job_key(identity_digest, index, command);
        let job_id = job_id_from_key(&identity_key);
        if entry.get("command_index").and_then(Value::as_u64) != Some(index as u64)
            || entry.get("command").and_then(Value::as_str) != Some(command.as_str())
            || entry.get("job_key").and_then(Value::as_str) != Some(key.as_str())
            || entry.get("job_id").and_then(Value::as_str) != Some(job_id.as_str())
        {
            return Err(SubmitError::Validation(format!(
                "run id {run_id} has a corrupt command-to-job mapping at index {index}"
            )));
        }
        let planned_value = entry
            .get("planned_job")
            .cloned()
            .ok_or_else(|| SubmitError::Validation("run entry has no planned job".into()))?;
        let job: Job = serde_json::from_value(planned_value.clone())
            .map_err(|error| SubmitError::Validation(format!("invalid planned job: {error}")))?;
        let mut effective = options.clone();
        effective.exclusive = effective.exclusive && !activation_extraction_must_share_gpu(command);
        let legacy_provenance;
        let expected_provenance = if legacy_request_digest.is_some() {
            legacy_provenance = SubmissionProvenance {
                created_at: job.created_at.clone(),
                submitted_by: provenance.submitted_by.clone(),
                submitted_from: provenance.submitted_from.clone(),
                submitter_app: provenance.submitter_app.clone(),
            };
            &legacy_provenance
        } else {
            &provenance
        };
        let mut expected = build_planned_job(
            command,
            &effective,
            &job_id,
            &resolved_hardware[index],
            expected_provenance,
        );
        expected.submission_request_digest = identity_digest.to_string();
        expected.submission_command_index = Some(index);
        if serde_json::to_value(&expected)
            .map_err(|error| SubmitError::Validation(error.to_string()))?
            != planned_value
        {
            return Err(SubmitError::Validation(format!(
                "run id {run_id} has a planned job not derivable from its request at index {index}"
            )));
        }
        let state = entry
            .get("state")
            .and_then(Value::as_str)
            .ok_or_else(|| SubmitError::Validation("run entry state is missing".into()))?;
        if !matches!(
            state,
            "planned" | "claimed" | "enqueuing" | "accepted" | "terminal" | "reaped"
        ) {
            return Err(SubmitError::Validation(format!(
                "run id {run_id} has invalid entry state {state}"
            )));
        }
        let outcome_job: Option<Job> = entry
            .get("outcome")
            .and_then(Value::as_object)
            .and_then(|outcome| outcome.get("job"))
            .cloned()
            .map(serde_json::from_value)
            .transpose()
            .map_err(|error| {
                SubmitError::Validation(format!("invalid terminal outcome: {error}"))
            })?;
        if matches!(state, "terminal" | "reaped") {
            let outcome = entry
                .get("outcome")
                .and_then(Value::as_object)
                .ok_or_else(|| SubmitError::Validation("terminal entry has no outcome".into()))?;
            if outcome.len() != 3
                || !["prefix", "recorded_at", "job"]
                    .into_iter()
                    .all(|field| outcome.contains_key(field))
            {
                return Err(SubmitError::Validation(
                    "terminal entry outcome has invalid fields".into(),
                ));
            }
            let prefix = outcome
                .get("prefix")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let terminal_job = outcome_job.as_ref().ok_or_else(|| {
                SubmitError::Validation("terminal entry has no retained job".into())
            })?;
            if !crate::queue::runs::TERMINAL_PREFIXES.contains(&prefix)
                || terminal_job.state != prefix
            {
                return Err(SubmitError::Validation(
                    "terminal entry prefix and retained job state disagree".into(),
                ));
            }
            let recorded_at = outcome
                .get("recorded_at")
                .and_then(Value::as_str)
                .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
                .map(|value| value.with_timezone(&Utc))
                .ok_or_else(|| {
                    SubmitError::Validation("terminal outcome recorded_at is invalid".into())
                })?;
            if recorded_at < manifest_created_at
                || recorded_at > Utc::now() + chrono::Duration::minutes(5)
            {
                return Err(SubmitError::Validation(
                    "terminal outcome recorded_at is outside the run lifetime".into(),
                ));
            }
            let terminal_at = match prefix {
                "failed" if terminal_job.completed_at.is_none() => {
                    terminal_job.failed_at.as_deref()
                }
                "completed" | "uploaded" | "cancelled" if terminal_job.failed_at.is_none() => {
                    terminal_job.completed_at.as_deref()
                }
                _ => None,
            }
            .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
            .map(|value| value.with_timezone(&Utc))
            .ok_or_else(|| {
                SubmitError::Validation(
                    "terminal retained job has contradictory terminal timestamps".into(),
                )
            })?;
            if terminal_at < manifest_created_at || terminal_at > recorded_at {
                return Err(SubmitError::Validation(
                    "terminal retained job timestamp is outside the outcome lifetime".into(),
                ));
            }
            if !crate::queue::runs::terminal_job_matches_entry(terminal_job, &job, run_id, index) {
                return Err(SubmitError::Validation(format!(
                    "stable job key {} belongs to different submission content",
                    job.job_id
                )));
            }
        } else if outcome_job.is_some() {
            return Err(SubmitError::Validation(
                "non-terminal entry unexpectedly carries an outcome".into(),
            ));
        }
        validated.push(ManifestEntry {
            planned_job: job,
            state: state.to_string(),
            outcome_job,
        });
    }
    Ok(validated)
}
