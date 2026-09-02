//! Job submission through compute.wisent.com or direct queue storage.
//!
//! The compute API key and repository/provider tokens are resolved from
//! Skarbiec. The API path posts to `{COMPUTE_API}/api/v1/instances`; the queue
//! path renders the startup script, writes it to internal queue storage and
//! the provider-neutral object namespace, then writes the queued job record.

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

use crate::catalog::GPU_SIZING;
use crate::config;
use crate::models::{
    activation_extraction_must_share_gpu, deprecated_activation_command_reason, Job, JobSecretRef,
};
use crate::queue::runs::RUN_PREFIX;
use crate::queue::storage::JobStorage;
use crate::queue::StorageError;

/// Directory the startup-script templates ship in (Python `TEMPLATE_DIR` =
/// `stado/templates/`).
/// Submission failure from validation, queue storage, or local rendering.
#[derive(Debug, thiserror::Error)]
pub enum SubmitError {
    #[error("{0}")]
    Validation(String),
    #[error(transparent)]
    Storage(#[from] StorageError),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResolvedHardwareProjection {
    pub gpu_mem_gb: i64,
    pub gpu_type: String,
    pub machine_type: String,
}

/// Every durable submission option. Callers start from
/// [`SubmitOptions::default`] and set what they need.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SubmitOptions {
    pub provider: String,
    pub batch_id: String,
    pub bucket: String,
    pub preemptible: bool,
    pub max_cost_per_hour_usd: f64,
    pub pin_to_provider: bool,
    pub priority: i64,
    pub deadline_at: Option<String>,
    pub repo: String,
    pub repo_ref: String,
    pub repo_workdir: String,
    pub repo_extras: String,
    pub gpu_type: String,
    pub vram_gb: i64,
    pub machine_type: String,
    /// Exact resolved hardware, used by durable replay/rerun to bypass
    /// mutable sizing catalogs. Normal interactive submissions leave it unset.
    pub resolved_hardware: Option<ResolvedHardwareProjection>,
    /// Operating system the job requires (`Job::platform_os`). Empty is no
    /// constraint; a native build declares the one platform whose binaries it
    /// can produce, and only a host of that platform claims it.
    pub platform_os: String,
    /// Architecture the job requires (`Job::architecture`). Empty is no
    /// constraint. See [`SubmitOptions::platform_os`].
    pub architecture: String,
    pub pre_command: String,
    pub apt_packages: Vec<String>,
    pub output_uri: String,
    pub verify_command: String,
    pub exclusive: bool,
    pub run_id: String,
    pub schedule_id: String,
    pub re_submission_of: String,
    pub yieldable: bool,
    pub yield_command: String,
    pub yield_grace_seconds: i64,
    pub pinned_host: String,
    pub secret_env: BTreeMap<String, JobSecretRef>,
    pub input_artifacts: Map<String, Value>,
    pub resolved_input_artifacts: Map<String, Value>,
}

impl Default for SubmitOptions {
    /// Stado defaults: no provider pin, `repo_extras="train"`,
    /// `yield_grace_seconds=120`, everything else empty/zero/false.
    fn default() -> Self {
        Self {
            provider: String::new(),
            batch_id: String::new(),
            bucket: String::new(),
            preemptible: false,
            max_cost_per_hour_usd: 0.0,
            pin_to_provider: false,
            priority: 0,
            deadline_at: None,
            repo: String::new(),
            repo_ref: String::new(),
            repo_workdir: String::new(),
            repo_extras: "train".into(),
            gpu_type: String::new(),
            vram_gb: 0,
            machine_type: String::new(),
            resolved_hardware: None,
            platform_os: String::new(),
            architecture: String::new(),
            pre_command: String::new(),
            apt_packages: vec![],
            output_uri: String::new(),
            verify_command: String::new(),
            exclusive: false,
            run_id: String::new(),
            schedule_id: String::new(),
            re_submission_of: String::new(),
            yieldable: false,
            yield_command: String::new(),
            yield_grace_seconds: 120,
            pinned_host: String::new(),
            secret_env: BTreeMap::new(),
            input_artifacts: Map::new(),
            resolved_input_artifacts: Map::new(),
        }
    }
}

/// The SKU the CPU branch of [`build_job`] writes: no accelerator was
/// asked for and the command sized to nothing. Named because it is also a
/// *readback* marker — `cli::job` recognizes a job that came out of that
/// branch by this machine_type, and must then resubmit with the routing
/// flags left empty rather than pinning them (pinning any of them flips
/// `caller_asked_for_gpu` and stamps an accelerator onto a CPU job).
///
/// It is a GCE name, kept because it is that readback marker, and it is
/// therefore **not** a portable VM size. `scheduler::dispatch::agent` refuses
/// to hand a machine type to a provider that does not name sizes that way, so
/// this marker cannot reach Azure as `hardwareProfile.vmSize`.
pub const CPU_MACHINE_TYPE: &str = "e2-standard-8";

/// Python `json.dumps(value, sort_keys=True, separators=(",", ":"))`:
/// compact separators, keys sorted recursively, non-ASCII escaped as
/// \uXXXX (Python's default `ensure_ascii=True`).
pub fn json_dumps_sorted_compact(value: &Value) -> String {
    fn sorted(value: &Value) -> Value {
        match value {
            Value::Object(map) => {
                let btree: BTreeMap<String, Value> = map
                    .iter()
                    .map(|(key, value)| (key.clone(), sorted(value)))
                    .collect();
                Value::Object(btree.into_iter().collect())
            }
            Value::Array(items) => Value::Array(items.iter().map(sorted).collect()),
            other => other.clone(),
        }
    }
    let compact = serde_json::to_string(&sorted(value)).expect("JSON serialization is infallible");
    crate::models::ensure_ascii(&compact)
}

/// `platform.node()` — cross-platform replacement for os.uname().nodename.
fn hostname() -> String {
    if let Ok(name) = std::env::var("HOSTNAME") {
        if !name.is_empty() {
            return name;
        }
    }
    std::process::Command::new("hostname")
        .output()
        .ok()
        .filter(|out| out.status.success())
        .map(|out| String::from_utf8_lossy(&out.stdout).trim().to_string())
        .filter(|name| !name.is_empty())
        .unwrap_or_default()
}

/// `$USER` or `$LOGNAME` (Python `os.environ.get("USER", "") or ...`).
fn submitter() -> String {
    let user = std::env::var("USER").unwrap_or_default();
    if !user.is_empty() {
        return user;
    }
    std::env::var("LOGNAME").unwrap_or_default()
}
fn digest_value(value: &Value) -> String {
    hex::encode(Sha256::digest(json_dumps_sorted_compact(value).as_bytes()))
}

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
fn submission_request(
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

#[derive(Debug)]
struct ManifestEntry {
    planned_job: Job,
    state: String,
    outcome_job: Option<Job>,
}

pub fn submission_job_key(request_digest: &str, index: usize, command: &str) -> String {
    digest_value(&serde_json::json!({
        "request_digest": request_digest,
        "command_index": index,
        "command": command,
    }))
}

/// The complete immutable submission projection. These are the only fields
/// excluded because Stado deliberately mutates them after admission:
/// lifecycle state/timestamps and provider attachment; retry, preemption and
/// yield counters; scheduler assignment/dispatch estimates; operator priority;
/// measured sizing/output observations.
pub(crate) fn immutable_job_projection(job: &Job) -> Value {
    let mut value = serde_json::to_value(job).expect("Job serialization is infallible");
    let object = value.as_object_mut().expect("Job serializes as an object");
    for field in [
        "state",
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

fn validate_recovered_job(job: &Job, planned: &Job, index: usize) -> Result<(), SubmitError> {
    if job.job_id != planned.job_id
        || job.submission_request_digest != planned.submission_request_digest
        || job.submission_command_index != Some(index)
        || immutable_job_projection(job) != immutable_job_projection(planned)
    {
        return Err(SubmitError::Validation(format!(
            "stable job key {} belongs to different submission content",
            planned.job_id
        )));
    }
    Ok(())
}

fn validate_run_manifest(
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
        let job_id = format!("job-{}", &identity_key[..24]);
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
            validate_recovered_job(terminal_job, &job, index)?;
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
pub(crate) fn validate_stored_run_manifest(
    manifest: &Value,
    run_id: &str,
) -> Result<(), SubmitError> {
    let request = manifest
        .get("request")
        .ok_or_else(|| SubmitError::Validation("run manifest request is missing".into()))?;
    let request_digest = digest_value(request);
    validate_run_manifest(manifest, run_id, request, &request_digest).map(|_| ())
}

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
            .ok_or_else(|| SubmitError::Validation(format!("run manifest {run_id} disappeared")))?;
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
            let old_job_id = format!("job-{}", &old_key[..24]);
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

enum EntryClaim {
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

async fn claim_entry(
    store: &JobStorage,
    path: &str,
    run_id: &str,
    request: &Value,
    request_digest: &str,
    index: usize,
    owner: &str,
) -> Result<EntryClaim, SubmitError> {
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

async fn checkpoint_accepted(
    store: &JobStorage,
    path: &str,
    run_id: &str,
    request: &Value,
    request_digest: &str,
    index: usize,
    job_id: &str,
    owner: &str,
) -> Result<(), SubmitError> {
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
                let job_id = format!("job-{}", &key[..24]);
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
    let mut accepted = Vec::with_capacity(planned.len());
    for index in 0..planned.len() {
        match claim_entry(
            &store,
            &path,
            &run_id,
            &request,
            &request_digest,
            index,
            &owner,
        )
        .await?
        {
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
                checkpoint_accepted(
                    &store,
                    &path,
                    &run_id,
                    &request,
                    &request_digest,
                    index,
                    &job.job_id,
                    &owner,
                )
                .await?;
                accepted.push(job);
            }
        }
    }
    Ok(accepted)
}

fn validate_submission(command: &str, options: &SubmitOptions) -> Result<(), SubmitError> {
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
    if options.yieldable && options.yield_command.trim().is_empty() {
        return Err(SubmitError::Validation(
            "yieldable=True requires a yield_command (the save-and-sync hook run on eviction)"
                .into(),
        ));
    }
    let repo = options.repo.trim();
    let repo_ref = options.repo_ref.trim();
    let full_commit_len = "0000000000000000000000000000000000000000".len();
    if repo.is_empty() {
        if !repo_ref.is_empty() {
            return Err(SubmitError::Validation(
                "repo_ref is valid only when repo is set".into(),
            ));
        }
    } else if repo_ref.len() != full_commit_len
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
        crate::object_store::ObjectRef::parse(&options.output_uri).map_err(|error| {
            SubmitError::Validation(format!(
                "output_uri must be a provider-neutral stado:// object URI: {error}"
            ))
        })?;
    }
    Ok(())
}

/// config::estimate_gpu_memory against the configured queue bucket
/// (Python's sizing scan always targets the global BUCKET, not the
/// submit-time bucket option), using the process-wide sizing caches. A
/// regex miss short-circuits to 0 without constructing the storage
/// handle, like Python returning before the sizing import does any GCS
/// work.
async fn estimate_gpu_mem(command: &str) -> Result<i64, SubmitError> {
    if crate::sizing::model_of(command).is_empty() {
        return Ok(0);
    }
    let store = default_store("").await?;
    Ok(config::estimate_gpu_memory(command, crate::sizing::global(), &store).await?)
}

/// Submit directly to Stado queue storage (no API server needed).
///
/// Sizing precedence (each layer overrides the previous):
///   1. estimate_gpu_memory(command) — model-name regex on the command,
///      the wisent-eval default. Falls back to 0 (CPU) if nothing matches.
///   2. vram_gb argument — caller-declared VRAM requirement. Skips the
///      regex when set, picks SKU via lookup_instance_type.
///   3. gpu_type argument — caller-pinned accelerator label
///      (e.g. "nvidia-l4"). Resolves to its tier's machine_type from
///      GPU_SIZING when machine_type is not also explicit.
///   4. machine_type argument — caller-pinned GCE machine type, taken
///      verbatim. Use this for non-cataloged SKUs.
#[derive(Debug, Clone)]
struct SubmissionProvenance {
    created_at: String,
    submitted_by: String,
    submitted_from: String,
    submitter_app: String,
}

async fn resolve_hardware(
    command: &str,
    options: &SubmitOptions,
) -> Result<ResolvedHardwareProjection, SubmitError> {
    if let Some(resolved) = options.resolved_hardware.as_ref() {
        return Ok(resolved.clone());
    }
    let caller_asked_for_gpu =
        !options.gpu_type.is_empty() || options.vram_gb > 0 || !options.machine_type.is_empty();
    let mut gpu_mem = if options.vram_gb > 0 {
        options.vram_gb
    } else {
        estimate_gpu_mem(command).await?
    };
    let (machine_type, gpu_type) = if !caller_asked_for_gpu && gpu_mem == 0 {
        (CPU_MACHINE_TYPE.into(), String::new())
    } else {
        let (inferred_machine, inferred_accel) =
            config::lookup_instance_type(&options.provider, gpu_mem);
        let gpu_type = if options.gpu_type.is_empty() {
            inferred_accel.to_string()
        } else {
            options.gpu_type.clone()
        };
        let machine_type = if !options.machine_type.is_empty() {
            options.machine_type.clone()
        } else if !options.gpu_type.is_empty() && options.vram_gb == 0 {
            let empty = BTreeMap::new();
            let sizing = GPU_SIZING.get(options.provider.as_str()).unwrap_or(&empty);
            match sizing
                .iter()
                .find(|(_, (_, accel))| *accel == options.gpu_type)
            {
                Some((mem, (machine, _))) => {
                    if gpu_mem == 0 {
                        gpu_mem = *mem;
                    }
                    machine.to_string()
                }
                None => inferred_machine.to_string(),
            }
        } else {
            inferred_machine.to_string()
        };
        (machine_type, gpu_type)
    };
    Ok(ResolvedHardwareProjection {
        gpu_mem_gb: gpu_mem,
        gpu_type,
        machine_type,
    })
}

fn build_planned_job(
    command: &str,
    options: &SubmitOptions,
    job_id: &str,
    hardware: &ResolvedHardwareProjection,
    provenance: &SubmissionProvenance,
) -> Job {
    let mut job = Job::new(job_id, command);
    job.created_at = provenance.created_at.clone();
    job.gpu_mem_gb = hardware.gpu_mem_gb;
    job.gpu_type = hardware.gpu_type.clone();
    job.machine_type = hardware.machine_type.clone();
    job.platform_os = options.platform_os.clone();
    job.architecture = options.architecture.clone();
    job.provider = options.provider.clone();
    job.batch_id = options.batch_id.clone();
    job.preemptible = options.preemptible;
    job.max_cost_per_hour_usd = options.max_cost_per_hour_usd;
    job.pin_to_provider = options.pin_to_provider;
    job.priority = options.priority;
    job.deadline_at = options.deadline_at.clone();
    job.submitted_by = provenance.submitted_by.clone();
    job.submitted_from = provenance.submitted_from.clone();
    job.submitted_via = "cli".into();
    job.run_id = options.run_id.clone();
    job.submitter_app = provenance.submitter_app.clone();
    job.repo = options.repo.clone();
    job.repo_ref = options.repo_ref.clone();
    job.repo_workdir = options.repo_workdir.clone();
    job.repo_extras = options.repo_extras.clone();
    job.pre_command = options.pre_command.clone();
    job.apt_packages = options.apt_packages.clone();
    job.output_uri = options.output_uri.clone();
    job.verify_command = options.verify_command.clone();
    job.exclusive = options.exclusive;
    job.schedule_id = options.schedule_id.clone();
    job.re_submission_of = options.re_submission_of.clone();
    job.yieldable = options.yieldable;
    job.yield_command = options.yield_command.clone();
    job.yield_grace_seconds = options.yield_grace_seconds;
    job.pinned_host = options.pinned_host.clone();
    job.secret_env = options.secret_env.clone();
    job.input_artifacts = options.input_artifacts.clone();
    job.resolved_input_artifacts = options.resolved_input_artifacts.clone();
    job
}

/// Construct the [`JobStorage`] handle the Python code builds as
/// `JobStorage(bucket or BUCKET)` — exposed for consumers (cancel, status)
/// that follow the same pattern.
pub async fn default_store(bucket: &str) -> Result<JobStorage, StorageError> {
    let bucket = if bucket.is_empty() {
        config::bucket()
    } else {
        bucket
    };
    JobStorage::with_bucket(bucket).await
}
