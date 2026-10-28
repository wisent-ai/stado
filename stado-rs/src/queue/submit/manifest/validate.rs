//! Proof that a stored run manifest is exactly the plan its immutable request
//! derives, entry by entry, including retained terminal outcomes.

mod outcome;

use chrono::{DateTime, Utc};
use serde_json::Value;

use crate::models::Job;
use crate::queue::submit::{
    build_planned_job, job_id_from_key, submission_input_digest, submission_job_key,
    submission_source_digest, ResolvedHardwareProjection, SubmissionProvenance, SubmitError,
    SubmitOptions,
};

use super::ManifestEntry;

/// Lowercase hex of a SHA-256 digest's length, measured from a digest.
fn lowercase_sha256_hex(value: &str) -> bool {
    use sha2::{Digest, Sha256};
    value.len() == hex::encode(Sha256::digest(b"")).len()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

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
    if legacy_request_digest.is_some_and(|digest| !lowercase_sha256_hex(digest)) {
        return Err(SubmitError::Validation(format!(
            "run id {run_id} has an invalid v2 identity digest"
        )));
    }
    let mut validated = Vec::with_capacity(entries.len());
    for (index, entry) in entries.iter().enumerate() {
        let command = &commands[index];
        let key = submission_job_key(request_digest, index, command);
        // A run migrated from v2 keeps the job identities that request gave it.
        let identity_digest = match legacy_request_digest {
            Some(legacy) => legacy,
            None => request_digest,
        };
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
        let job: Job = serde_json::from_value(planned_value)
            .map_err(|error| SubmitError::Validation(format!("invalid planned job: {error}")))?;
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
            &options,
            &job_id,
            &resolved_hardware[index],
            expected_provenance,
        );
        expected.submission_request_digest = identity_digest.to_string();
        expected.submission_command_index = Some(index);
        // The stored plan is compared as this build's model reads it, not
        // byte for byte: a job field added since the run was written
        // serialises on the re-derived job and not on the stored one, which
        // made every older run "not derivable", and with it every terminal
        // outcome of its jobs unrecordable. What the model does not carry is
        // not part of the plan's identity.
        let stored = serde_json::to_value(&job)
            .map_err(|error| SubmitError::Validation(error.to_string()))?;
        if serde_json::to_value(&expected)
            .map_err(|error| SubmitError::Validation(error.to_string()))?
            != stored
        {
            return Err(SubmitError::Validation(format!(
                "run id {run_id} has a planned job not derivable from its request at index {index}"
            )));
        }
        let state = entry
            .get("state")
            .and_then(Value::as_str)
            .ok_or_else(|| SubmitError::Validation("run entry state is missing".into()))?;
        let state: super::EntryState = serde_json::from_value(Value::String(state.to_string()))
            .map_err(|_| {
                SubmitError::Validation(format!("run id {run_id} has invalid entry state {state}"))
            })?;
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
        if state.ended() {
            outcome::terminal_outcome(
                entry,
                outcome_job.as_ref(),
                &job,
                run_id,
                index,
                manifest_created_at,
            )?;
        } else if outcome_job.is_some() {
            return Err(SubmitError::Validation(
                "non-terminal entry unexpectedly carries an outcome".into(),
            ));
        }
        validated.push(ManifestEntry {
            planned_job: job,
            state,
            outcome_job,
        });
    }
    Ok(validated)
}
