//! Job submission through compute.wisent.com or direct queue storage.
//!
//! The compute API key and repository/provider tokens are resolved from
//! Skarbiec. The API path posts to `{COMPUTE_API}/api/v1/instances`; the queue
//! path renders the startup script, writes it to internal queue storage and
//! the provider-neutral object namespace, then writes the queued job record.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

use crate::catalog::GPU_SIZING;
use crate::config;
use crate::models::{
    activation_extraction_must_share_gpu, deprecated_activation_command_reason, Job, JobSecretRef,
};
use crate::queue::runs::{generate_run_id, ALL_PREFIXES, RUN_PREFIX};
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

/// Every `submit_job` keyword argument from the Python signature, with the
/// same defaults. Callers start from [`SubmitOptions::default`] and set
/// what they need.
#[derive(Debug, Clone, Serialize, Deserialize)]
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

/// The SKU the CPU branch of [`submit_to_queue`] writes: no accelerator was
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

/// `os.urandom(4).hex()` — 4 random bytes as 8 hex chars.
pub fn generate_job_id() -> String {
    hex::encode(&uuid::Uuid::new_v4().as_bytes()[..4])
}

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

/// Canonical semantic request. Submitter display fields and random IDs are
/// deliberately absent; every option that changes placement, source, secrets,
/// inputs or execution is included.
pub fn submission_request(commands: &[String], options: &SubmitOptions) -> Result<Value, SubmitError> {
    let options_value = serde_json::to_value(options)
        .map_err(|error| SubmitError::Validation(format!("serialize submission options: {error}")))?;
    Ok(serde_json::json!({
        "schema": "stado.submission-request.v2",
        "commands": commands,
        "effective_bucket": if options.bucket.is_empty() { config::bucket() } else { options.bucket.as_str() },
        "options": options_value,
    }))
}

pub fn submission_request_digest(
    commands: &[String],
    options: &SubmitOptions,
) -> Result<String, SubmitError> {
    Ok(digest_value(&submission_request(commands, options)?))
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

fn validate_run_manifest(
    manifest: &Value,
    run_id: &str,
    request: &Value,
    request_digest: &str,
) -> Result<Vec<Job>, SubmitError> {
    if manifest.get("schema").and_then(Value::as_str) != Some("stado.run-submission.v2")
        || manifest.get("run_id").and_then(Value::as_str) != Some(run_id)
        || manifest.get("request_digest").and_then(Value::as_str) != Some(request_digest)
        || manifest.get("request") != Some(request)
    {
        return Err(SubmitError::Validation(format!(
            "run id {run_id} already belongs to a different or legacy submission request"
        )));
    }
    let entries = manifest
        .get("entries")
        .and_then(Value::as_array)
        .ok_or_else(|| SubmitError::Validation("run manifest entries are missing".into()))?;
    let mut jobs = Vec::with_capacity(entries.len());
    for (index, entry) in entries.iter().enumerate() {
        if entry.get("command_index").and_then(Value::as_u64) != Some(index as u64) {
            return Err(SubmitError::Validation(format!(
                "run id {run_id} has a non-canonical command mapping"
            )));
        }
        let job: Job = serde_json::from_value(
            entry
                .get("planned_job")
                .cloned()
                .ok_or_else(|| SubmitError::Validation("run entry has no planned job".into()))?,
        )
        .map_err(|error| SubmitError::Validation(format!("invalid planned job: {error}")))?;
        if entry.get("job_id").and_then(Value::as_str) != Some(job.job_id.as_str())
            || entry.get("command").and_then(Value::as_str) != Some(job.command.as_str())
            || job.run_id != run_id
            || job.submission_request_digest != request_digest
            || job.submission_command_index != Some(index)
        {
            return Err(SubmitError::Validation(format!(
                "run id {run_id} has a corrupt planned command-to-job mapping"
            )));
        }
        jobs.push(job);
    }
    Ok(jobs)
}

async fn checkpoint_accepted(
    store: &JobStorage,
    run_id: &str,
    request_digest: &str,
    index: usize,
    job_id: &str,
) -> Result<(), SubmitError> {
    let path = format!("{RUN_PREFIX}/{run_id}.json");
    for _ in 0..16 {
        let versioned = store
            .read_text_versioned(&path)
            .await?
            .ok_or_else(|| SubmitError::Validation(format!("run manifest {run_id} disappeared")))?;
        let mut manifest: Value = serde_json::from_str(&versioned.content)
            .map_err(|error| SubmitError::Validation(format!("invalid run manifest: {error}")))?;
        if manifest.get("request_digest").and_then(Value::as_str) != Some(request_digest) {
            return Err(SubmitError::Validation(format!(
                "run id {run_id} changed to a different request"
            )));
        }
        let entry = manifest
            .get_mut("entries")
            .and_then(Value::as_array_mut)
            .and_then(|entries| entries.get_mut(index))
            .and_then(Value::as_object_mut)
            .ok_or_else(|| SubmitError::Validation("run checkpoint entry is missing".into()))?;
        if entry.get("job_id").and_then(Value::as_str) != Some(job_id) {
            return Err(SubmitError::Validation(
                "run checkpoint job mapping changed".into(),
            ));
        }
        if entry.get("state").and_then(Value::as_str) == Some("accepted") {
            return Ok(());
        }
        entry.insert("state".into(), Value::from("accepted"));
        entry.insert("accepted_at".into(), Value::from(chrono::Utc::now().to_rfc3339()));
        match store
            .compare_and_swap_text(
                &path,
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
    for prefix in ALL_PREFIXES {
        if let Some(job) = store.read_job(prefix, job_id).await? {
            return Ok(Some(job));
        }
    }
    Ok(None)
}

fn validate_recovered_job(
    job: &Job,
    planned: &Job,
    request_digest: &str,
    index: usize,
) -> Result<(), SubmitError> {
    if job.job_id != planned.job_id
        || job.command != planned.command
        || job.run_id != planned.run_id
        || job.repo != planned.repo
        || job.repo_ref != planned.repo_ref
        || job.output_uri != planned.output_uri
        || job.submission_request_digest != request_digest
        || job.submission_command_index != Some(index)
    {
        return Err(SubmitError::Validation(format!(
            "stable job key {} belongs to different submission content",
            planned.job_id
        )));
    }
    Ok(())
}

/// Persist a complete immutable plan before any queue write, then create each
/// stable command job exactly once and CAS-checkpoint acceptance in order.
pub async fn submit_batch(
    commands: &[String],
    options: &SubmitOptions,
) -> Result<Vec<Job>, SubmitError> {
    if commands.is_empty() {
        return Err(SubmitError::Validation("at least one command is required".into()));
    }
    let mut options = options.clone();
    if options.run_id.is_empty() {
        options.run_id = generate_run_id();
    }
    for command in commands {
        validate_submission(command, &options)?;
    }
    let run_id = options.run_id.clone();
    let request = submission_request(commands, &options)?;
    let request_digest = digest_value(&request);
    let bucket = if options.bucket.is_empty() {
        config::bucket()
    } else {
        options.bucket.as_str()
    };
    let store = JobStorage::with_bucket(bucket).await?;
    let path = format!("{RUN_PREFIX}/{run_id}.json");

    let manifest = match store.download_text(&path).await? {
        Some(raw) => serde_json::from_str::<Value>(&raw)
            .map_err(|error| SubmitError::Validation(format!("invalid run manifest: {error}")))?,
        None => {
            let mut entries = Vec::with_capacity(commands.len());
            let mut job_ids = Vec::with_capacity(commands.len());
            for (index, command) in commands.iter().enumerate() {
                let key = digest_value(&serde_json::json!({
                    "request_digest": request_digest,
                    "command_index": index,
                    "command": command,
                }));
                let job_id = format!("job-{}", &key[..24]);
                let mut effective = options.clone();
                effective.exclusive =
                    effective.exclusive && !activation_extraction_must_share_gpu(command);
                let mut job = build_job(command, &effective, &job_id).await?;
                job.submission_request_digest = request_digest.clone();
                job.submission_command_index = Some(index);
                job_ids.push(Value::from(job_id.clone()));
                entries.push(serde_json::json!({
                    "command_index": index,
                    "command": command,
                    "job_key": key,
                    "job_id": job_id,
                    "state": "planned",
                    "planned_job": job,
                }));
            }
            let candidate = serde_json::json!({
                "schema": "stado.run-submission.v2",
                "run_id": run_id,
                "request_digest": request_digest,
                "source_digest": submission_source_digest(&options),
                "input_digest": submission_input_digest(commands, &options),
                "request": request,
                "created_at": chrono::Utc::now().to_rfc3339(),
                "submitter_app": std::env::var("WC_SUBMITTER_APP").unwrap_or_default(),
                "submitted_by": submitter(),
                "submitted_from": hostname(),
                "n_jobs": commands.len(),
                "job_ids": job_ids,
                "commands": commands,
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
                let raw = store
                    .download_text(&path)
                    .await?
                    .ok_or_else(|| SubmitError::Validation("run manifest creation raced and disappeared".into()))?;
                serde_json::from_str::<Value>(&raw)
                    .map_err(|error| SubmitError::Validation(format!("invalid run manifest: {error}")))?
            }
        }
    };
    let planned = validate_run_manifest(&manifest, &run_id, &request, &request_digest)?;
    let mut accepted = Vec::with_capacity(planned.len());
    for (index, planned_job) in planned.iter().enumerate() {
        let job = if let Some(existing) = find_job(&store, &planned_job.job_id).await? {
            validate_recovered_job(&existing, planned_job, &request_digest, index)?;
            existing
        } else if store.create_queued_job_if_absent(planned_job).await? {
            planned_job.clone()
        } else {
            let existing = find_job(&store, &planned_job.job_id)
                .await?
                .ok_or_else(|| {
                    SubmitError::Validation(format!(
                        "stable job {} was concurrently created but is unreadable",
                        planned_job.job_id
                    ))
                })?;
            validate_recovered_job(&existing, planned_job, &request_digest, index)?;
            existing
        };
        checkpoint_accepted(&store, &run_id, &request_digest, index, &job.job_id).await?;
        accepted.push(job);
    }
    Ok(accepted)
}

fn validate_submission(command: &str, options: &SubmitOptions) -> Result<(), SubmitError> {
    if command.trim().is_empty() {
        return Err(SubmitError::Validation("command cannot be empty".into()));
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

/// Submit a job through Stado's provider-neutral queue.
pub async fn submit_job(command: &str, options: &SubmitOptions) -> Result<Job, SubmitError> {
    validate_submission(command, options)?;
    let options = SubmitOptions {
        exclusive: options.exclusive && !activation_extraction_must_share_gpu(command),
        ..options.clone()
    };
    submit_to_queue(command, &options).await
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
async fn build_job(
    command: &str,
    options: &SubmitOptions,
    job_id: &str,
) -> Result<Job, SubmitError> {
    let caller_asked_for_gpu =
        !options.gpu_type.is_empty() || options.vram_gb > 0 || !options.machine_type.is_empty();
    let mut gpu_mem = if options.vram_gb > 0 {
        options.vram_gb
    } else {
        estimate_gpu_mem(command).await?
    };

    let machine_type: String;
    let accel_type: String;
    if !caller_asked_for_gpu && gpu_mem == 0 {
        // CPU path — no GPU requirements, no regex hit. Same as pre-0.4.122.
        machine_type = CPU_MACHINE_TYPE.into();
        accel_type = String::new();
    } else {
        let (inferred_machine, inferred_accel) =
            config::lookup_instance_type(&options.provider, gpu_mem);
        accel_type = if options.gpu_type.is_empty() {
            inferred_accel.to_string()
        } else {
            options.gpu_type.clone()
        };
        if !options.machine_type.is_empty() {
            // caller-pinned, take verbatim
            machine_type = options.machine_type.clone();
        } else if !options.gpu_type.is_empty() && options.vram_gb == 0 {
            // Caller pinned the accelerator but not the size — pick the
            // machine_type from GPU_SIZING by matching accel label.
            let empty = BTreeMap::new();
            let sizing = GPU_SIZING.get(options.provider.as_str()).unwrap_or(&empty);
            let matched = sizing
                .iter()
                .find(|(_, (_, accel))| *accel == options.gpu_type);
            match matched {
                Some((mem, (machine, _))) => {
                    machine_type = machine.to_string();
                    if gpu_mem == 0 {
                        gpu_mem = *mem;
                    }
                }
                None => machine_type = inferred_machine.to_string(),
            }
        } else {
            machine_type = inferred_machine.to_string();
        }
    }

    // priority stays user-controlled. Makespan-optimization happens in
    // the coordinator's centralized matcher (see _assign_jobs_to_agents
    // in coordinator.py), not by mutating the priority field at submit
    // time.
    let mut job = Job::new(job_id, command);
    job.gpu_mem_gb = gpu_mem;
    job.gpu_type = accel_type;
    job.machine_type = machine_type;
    job.platform_os = options.platform_os.clone();
    job.architecture = options.architecture.clone();
    job.provider = options.provider.clone();
    job.batch_id = options.batch_id.clone();
    job.preemptible = options.preemptible;
    job.max_cost_per_hour_usd = options.max_cost_per_hour_usd;
    job.pin_to_provider = options.pin_to_provider;
    job.priority = options.priority;
    job.deadline_at = options.deadline_at.clone();
    job.submitted_by = submitter();
    job.submitted_from = hostname();
    job.submitted_via = "cli".into();
    job.run_id = options.run_id.clone();
    job.submitter_app = std::env::var("WC_SUBMITTER_APP").unwrap_or_default();
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

    Ok(job)
}

async fn submit_to_queue(command: &str, options: &SubmitOptions) -> Result<Job, SubmitError> {
    let bucket = if options.bucket.is_empty() {
        config::bucket().to_string()
    } else {
        options.bucket.clone()
    };
    let job = build_job(command, options, &generate_job_id()).await?;
    let store = JobStorage::with_bucket(&bucket).await?;
    store.write_job("queue", &job).await?;
    Ok(job)
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
