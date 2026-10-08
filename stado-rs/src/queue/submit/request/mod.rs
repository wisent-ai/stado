//! Request validation: the run id and per-command checks a submission must
//! pass, the canonical semantic request derived from them, and the readback
//! check that a recovered job is the planned job. [`identity`] holds the
//! digests and stable keys that name this request and its jobs.

use serde_json::Value;

use crate::config;
use crate::models::{deprecated_activation_command_reason, Job};

use super::{ResolvedHardwareProjection, SubmitError, SubmitOptions};

mod identity;

pub use identity::{
    is_canonical_job_id, stable_run_id, submission_input_digest, submission_job_key,
    submission_source_digest,
};

pub(crate) use identity::immutable_job_projection;
pub(in crate::queue::submit) use identity::job_id_from_key;

pub fn validate_run_id(run_id: &str) -> Result<(), SubmitError> {
    if run_id.is_empty()
        || run_id.len() > 160
        || matches!(run_id, "." | "..")
        || !run_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
    {
        return Err(SubmitError::Validation(
            "run id must be 1-160 ASCII letters, digits, '.', '_' or '-'".into(),
        ));
    }
    Ok(())
}

/// Canonical semantic request. Submitter display fields and random IDs are
/// deliberately absent; every option that changes placement, source, secrets,
/// Canonical semantic request. The per-command resolved hardware projection
/// makes validation independent of mutable sizing catalogs while every other
/// execution field is derived from `options`.
pub(super) fn submission_request(
    commands: &[String],
    options: &SubmitOptions,
    resolved_hardware: &[ResolvedHardwareProjection],
) -> Result<Value, SubmitError> {
    let options_value = serde_json::to_value(options).map_err(|error| {
        SubmitError::Validation(format!("serialize submission options: {error}"))
    })?;
    Ok(serde_json::json!({
        "schema": "stado.submission-request.v3",
        "commands": commands,
        "effective_bucket": if options.bucket.is_empty() { config::bucket() } else { options.bucket.as_str() },
        "options": options_value,
        "resolved_hardware": resolved_hardware,
    }))
}

pub(super) fn validate_recovered_job(
    job: &Job,
    planned: &Job,
    index: usize,
) -> Result<(), SubmitError> {
    let same_request = job.job_id == planned.job_id
        && job.submission_request_digest == planned.submission_request_digest
        && job.submission_command_index == Some(index);
    // A job that has ended was planned by the binary that admitted it. The
    // request digest is the submission's identity; the rest of the projection
    // is what that binary derived from it — its defaults, its plan time — and
    // a later binary derives it differently: once the boot disk, preempt and
    // yield defaults were removed (8e11ce2d), every replay of a job admitted
    // before was refused as "different submission content" and its build could
    // never read how it ended (33df63e9). An ended job of the same request is
    // that submission's job; a live one must still match exactly, because it
    // is about to run what was planned.
    let ended = crate::queue::runs::TERMINAL_PREFIXES.contains(&job.state.as_str());
    if !same_request
        || (!ended && immutable_job_projection(job) != immutable_job_projection(planned))
    {
        return Err(SubmitError::Validation(format!(
            "stable job key {} belongs to different submission content",
            planned.job_id
        )));
    }
    Ok(())
}

pub(super) fn validate_submission(
    command: &str,
    options: &SubmitOptions,
) -> Result<(), SubmitError> {
    if command.trim().is_empty() {
        return Err(SubmitError::Validation("command cannot be empty".into()));
    }
    if command.len() > 1024 * 1024 {
        return Err(SubmitError::Validation(
            "command exceeds the 1 MiB durable manifest limit".into(),
        ));
    }
    if !options.max_cost_per_hour_usd.is_finite() || options.max_cost_per_hour_usd < 0.0 {
        return Err(SubmitError::Validation(
            "max_cost_per_hour_usd must be finite and nonnegative".into(),
        ));
    }
    if !crate::queue::listing::priority_fits(options.priority) {
        return Err(SubmitError::Validation(format!(
            "priority {} does not fit the queue's marker key; it must be between 0 and {}",
            options.priority,
            crate::queue::listing::PRIORITY_LIMIT
        )));
    }
    if options.yieldable && options.yield_command.trim().is_empty() {
        return Err(SubmitError::Validation(
            "yieldable=True requires a yield_command (the save-and-sync hook run on eviction)"
                .into(),
        ));
    }
    let repo = options.repo.trim();
    let repo_ref = options.repo_ref.trim();
    if repo.is_empty() {
        if !repo_ref.is_empty() {
            return Err(SubmitError::Validation(
                "repo_ref is valid only when repo is set".into(),
            ));
        }
    } else if repo_ref.len() != 40
        || !repo_ref
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(SubmitError::Validation(
            "repository workloads require repo_ref as a full lowercase 40-hex commit".into(),
        ));
    }
    let reason = deprecated_activation_command_reason(command);
    if !reason.is_empty() {
        return Err(SubmitError::Validation(reason.into()));
    }
    if !options.output_uri.trim().is_empty() {
        crate::remote::object_store::ObjectRef::parse(&options.output_uri).map_err(|error| {
            SubmitError::Validation(format!(
                "output_uri must be a provider-neutral stado:// object URI: {error}"
            ))
        })?;
    }
    Ok(())
}
