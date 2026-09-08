//! Submission identity: the stable run id a caller retains, the source and
//! input digests of a request, the stable per-command job key and job id, and
//! the immutable job projection every readback compares against.

use serde_json::Value;

use crate::models::Job;
use crate::queue::submit::{digest_value, SubmitOptions};

/// Derive a path-safe run id from a caller-retained domain token. Retrying the
/// same operation must pass the same token; distinct operations must not share
/// one. The original scope is included in the digest; its display fragment is
/// sanitized and therefore cannot escape the runs namespace.
pub fn stable_run_id(scope: &str, token: &str) -> String {
    let digest = digest_value(&serde_json::json!({
        "scope": scope,
        "token": token,
    }));
    let mut label = scope
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_') {
                character
            } else {
                '-'
            }
        })
        .take(32)
        .collect::<String>();
    label = label.trim_matches('-').to_string();
    if label.is_empty() {
        label = "scope".into();
    }
    format!("run-{label}-{}", &digest[..24])
}

pub fn submission_source_digest(options: &SubmitOptions) -> String {
    digest_value(&serde_json::json!({
        "repo": options.repo,
        "repo_ref": options.repo_ref,
        "repo_workdir": options.repo_workdir,
        "repo_extras": options.repo_extras,
        "pre_command": options.pre_command,
        "apt_packages": options.apt_packages,
    }))
}

pub fn submission_input_digest(commands: &[String], options: &SubmitOptions) -> String {
    digest_value(&serde_json::json!({
        "commands": commands,
        "secret_env": options.secret_env,
        "input_artifacts": options.input_artifacts,
        "resolved_input_artifacts": options.resolved_input_artifacts,
    }))
}

pub fn submission_job_key(request_digest: &str, index: usize, command: &str) -> String {
    digest_value(&serde_json::json!({
        "request_digest": request_digest,
        "command_index": index,
        "command": command,
    }))
}

/// Prefix and digest width of every job id emitted by queue submission.
pub const JOB_ID_PREFIX: &str = "job-";
pub const JOB_ID_HEX_LEN: usize = 24;

/// Whether `job_id` is byte-for-byte in the form queue submission emits.
pub fn is_canonical_job_id(job_id: &str) -> bool {
    job_id.strip_prefix(JOB_ID_PREFIX).is_some_and(|suffix| {
        suffix.len() == JOB_ID_HEX_LEN
            && suffix
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    })
}

pub(in crate::queue::submit) fn job_id_from_key(key: &str) -> String {
    format!("{JOB_ID_PREFIX}{}", &key[..JOB_ID_HEX_LEN])
}

/// The complete immutable submission projection. These are the only fields
/// excluded because Stado deliberately mutates them after admission:
/// lifecycle state/timestamps; optimizer-owned provider placement; retry,
/// preemption and yield counters; scheduler assignment/dispatch estimates;
/// operator priority; measured sizing/output observations. The durable request
/// digest still guards every caller-supplied option, including provider pins.
pub(crate) fn immutable_job_projection(job: &Job) -> Value {
    let mut value = serde_json::to_value(job).expect("Job serialization is infallible");
    let object = value.as_object_mut().expect("Job serializes as an object");
    for field in [
        "state",
        "provider",
        "pin_to_provider",
        "started_at",
        "completed_at",
        "lease_expires_at",
        "failed_at",
        "instance_ref",
        "restarts",
        "last_restart",
        "error",
        "preempt_count",
        "priority",
        "dispatch_attempts",
        "last_dispatch_attempt",
        "assigned_to",
        "provider",
        "pin_to_provider",
        "runtime_seconds_estimate",
        "gpu_mem_gb",
        "peak_vram_gb",
        "peak_vram_per_gpu",
        "yield_count",
        "artifact_paths",
    ] {
        object.remove(field);
    }
    value
}
