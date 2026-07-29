//! Job submission through compute.wisent.com or direct queue storage.
//!
//! The compute API key and repository/provider tokens are resolved from
//! Skarbiec. The API path posts to `{COMPUTE_API}/api/v1/instances`; the queue
//! path renders the startup script, writes it to internal queue storage and
//! the provider-neutral object namespace, then writes the queued job record.

use std::collections::BTreeMap;

use futures::StreamExt;
use serde_json::{Map, Value};

use crate::catalog::GPU_SIZING;
use crate::config;
use crate::models::{
    activation_extraction_must_share_gpu, deprecated_activation_command_reason, Job, JobSecretRef,
};
use crate::queue::runs::{generate_run_id, write_run_manifest, RunManifest};
use crate::queue::storage::JobStorage;
use crate::queue::StorageError;

/// Directory the startup-script templates ship in (Python `TEMPLATE_DIR` =
/// `stado/templates/`).
#[cfg(test)]
fn template_dir() -> std::path::PathBuf {
    crate::data_dir().join("templates")
}

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
#[derive(Debug, Clone)]
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
pub const CPU_MACHINE_TYPE: &str = "e2-standard-8";

/// `os.urandom(4).hex()` — 4 random bytes as 8 hex chars.
pub fn generate_job_id() -> String {
    hex::encode(&uuid::Uuid::new_v4().as_bytes()[..4])
}

/// Render a startup-script template: naive sequential `${KEY}` replacement,
/// exactly Python `str.replace(f"${{{key}}}", str(value))` per variable.
/// Variables not present in the template are ignored; `${...}` placeholders
/// with no matching variable are left untouched (Python parity).
#[cfg(test)]
fn render_template(
    template_name: &str,
    variables: &[(String, String)],
) -> Result<String, SubmitError> {
    let mut content = std::fs::read_to_string(template_dir().join(template_name))?;
    for (key, value) in variables {
        content = content.replace(&format!("${{{key}}}"), value);
    }
    Ok(content)
}

/// Bash that clones repo into $WORK/{workdir} and pip-installs its extras
/// so the user's command can `cd {workdir} && python -m foo` directly.
/// Returns empty string when no repo was requested.
#[cfg(test)]
fn render_repo_block(repo: &str, workdir: &str, extras: &str) -> String {
    if repo.is_empty() {
        return String::new();
    }
    // Default workdir = repo basename without .git
    let workdir = if workdir.is_empty() {
        let basename = repo.trim_end_matches('/').rsplit('/').next().unwrap_or("");
        basename
            .strip_suffix(".git")
            .unwrap_or(basename)
            .to_string()
    } else {
        workdir.to_string()
    };
    let install = if extras.is_empty() {
        String::new()
    } else {
        format!("pip install -e '.[{extras}]'")
    };
    format!("git clone --depth 1 {repo} {workdir}\ncd {workdir}\n{install}\ncd $WORK\n")
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

/// Submit many commands concurrently as one run.
///
/// Generates one run_id for the whole invocation, threads it onto every
/// job, then writes the immutable runs/<run_id>.json manifest with all
/// member job_ids. The compute-API path skips the manifest since it has no
/// queue storage to track against.
///
/// Batches of more than 4 commands fan out 64 ways (Python
/// `ThreadPoolExecutor(max_workers=64)` → `buffer_unordered(64)`); smaller
/// batches submit sequentially. Returns the submitted jobs (the Python
/// `return_jobs=True` form — the CLI's single-job path echoes the id so
/// callers can watch the job, not the batch).
pub async fn submit_batch(
    commands: &[String],
    options: &SubmitOptions,
) -> Result<Vec<Job>, SubmitError> {
    let mut options = options.clone();
    if options.run_id.is_empty() {
        options.run_id = generate_run_id();
    }
    let run_id = options.run_id.clone();

    let jobs: Vec<Job> = if commands.len() <= 4 {
        let mut out = Vec::with_capacity(commands.len());
        for command in commands {
            out.push(submit_job(command, &options).await?);
        }
        out
    } else {
        let results: Vec<Result<Job, SubmitError>> = futures::stream::iter(commands)
            .map(|command| submit_job(command, &options))
            .buffer_unordered(64)
            .collect()
            .await;
        results.into_iter().collect::<Result<Vec<_>, _>>()?
    };

    let bucket = if options.bucket.is_empty() {
        config::bucket()
    } else {
        options.bucket.as_str()
    };
    let store = JobStorage::with_bucket(bucket).await?;
    write_run_manifest(
        &store,
        &RunManifest {
            run_id: &run_id,
            name: Some(&std::env::var("WC_RUN_NAME").unwrap_or_default()),
            submitter_app: Some(&std::env::var("WC_SUBMITTER_APP").unwrap_or_default()),
            submitted_by: &submitter(),
            submitted_from: &hostname(),
            commands,
            job_ids: &jobs
                .iter()
                .map(|job| job.job_id.clone())
                .collect::<Vec<_>>(),
        },
    )
    .await?;
    Ok(jobs)
}

/// Submit a job through Stado's provider-neutral queue.
pub async fn submit_job(command: &str, options: &SubmitOptions) -> Result<Job, SubmitError> {
    // Cooperative-yield contract: a yieldable job MUST declare how to save
    // and step aside. No silent kill-and-lose-progress path — refuse here so
    // the error surfaces at submit, not mid-eviction in prod.
    if options.yieldable && options.yield_command.trim().is_empty() {
        return Err(SubmitError::Validation(
            "yieldable=True requires a yield_command (the save-and-sync hook \
             run on eviction). Pass --on-yield '<command>' or drop --yieldable."
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
async fn submit_to_queue(command: &str, options: &SubmitOptions) -> Result<Job, SubmitError> {
    let bucket = if options.bucket.is_empty() {
        config::bucket().to_string()
    } else {
        options.bucket.clone()
    };
    let job_id = generate_job_id();

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
    let mut job = Job::new(&job_id, command);
    job.gpu_mem_gb = gpu_mem;
    job.gpu_type = accel_type;
    job.machine_type = machine_type;
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::queue::local_file::LocalBackend;
    use std::sync::Arc;

    #[test]
    fn job_id_is_8_hex_chars() {
        let id = generate_job_id();
        assert_eq!(id.len(), 8);
        assert!(id.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn repo_block_matches_python_layout() {
        assert_eq!(render_repo_block("", "", ""), "");
        let block = render_repo_block("https://github.com/org/repo.git", "", "train");
        assert_eq!(
            block,
            "git clone --depth 1 https://github.com/org/repo.git repo\ncd repo\npip install -e '.[train]'\ncd $WORK\n"
        );
        // Explicit workdir + empty extras skips the install line content.
        let block = render_repo_block("https://github.com/org/repo", "wd", "");
        assert_eq!(
            block,
            "git clone --depth 1 https://github.com/org/repo wd\ncd wd\n\ncd $WORK\n"
        );
    }

    #[test]
    fn compact_sorted_json_matches_python_dumps() {
        let value = serde_json::json!({"b": 1, "a": {"z": true, "y": "ż"}, "c": [2, 1]});
        assert_eq!(
            json_dumps_sorted_compact(&value),
            "{\"a\":{\"y\":\"\\u017c\",\"z\":true},\"b\":1,\"c\":[2,1]}"
        );
    }

    #[tokio::test]
    async fn yieldable_without_command_is_refused() {
        let options = SubmitOptions {
            yieldable: true,
            ..Default::default()
        };
        let err = submit_job("echo hi", &options).await.unwrap_err();
        assert!(
            err.to_string()
                .starts_with("yieldable=True requires a yield_command"),
            "{err}"
        );
    }

    #[tokio::test]
    async fn deprecated_entrypoint_is_refused() {
        let err = submit_job(
            "python -m wisent.scripts.activations.extract_and_upload --x",
            &SubmitOptions::default(),
        )
        .await
        .unwrap_err();
        assert!(err.to_string().starts_with("refusing deprecated"), "{err}");
    }

    #[tokio::test]
    async fn template_rendering_substitutes_only_known_variables() {
        let script = render_template(
            "startup_gpu_agent.sh",
            &[
                ("WC_BUCKET".into(), "fixture-bucket".into()),
                ("STADO_RELEASE_VERSION".into(), "fixture-release".into()),
            ],
        )
        .unwrap();
        assert!(
            script.contains("export WC_BUCKET=\"fixture-bucket\""),
            "{script}"
        );
        assert!(
            script.contains("RELEASE_VERSION=\"fixture-release\""),
            "{script}"
        );
        assert!(!script.contains("${WC_BUCKET}"), "{script}");
        assert!(!script.contains("${STADO_RELEASE_VERSION}"), "{script}");
        // A placeholder without a supplied variable stays untouched.
        assert!(script.contains("${WC_STORAGE_BACKEND}"), "{script}");
    }

    #[tokio::test]
    async fn gcs_submit_writes_script_and_job() {
        let dir = tempfile::tempdir().unwrap();
        let store = JobStorage::with_backend(
            Arc::new(LocalBackend::new(dir.path().to_str().unwrap()).unwrap()),
            "local",
        );
        // Exercise the Job construction half via the public submit_job by
        // pointing the backend env at the temp dir is not possible here
        // (config LazyLock); this test instead verifies the template + job
        // wiring indirectly through render_template and the store facade.
        // The end-to-end path is covered by tests/cli_local.rs.
        let mut job = Job::new("abcd1234", "echo hi");
        job.gpu_mem_gb = 0;
        job.machine_type = "e2-standard-8".into();
        store
            .upload_script("abcd1234", "#!/bin/bash\n")
            .await
            .unwrap();
        store.write_job("queue", &job).await.unwrap();
        let back = store.read_job("queue", "abcd1234").await.unwrap().unwrap();
        assert_eq!(back.machine_type, "e2-standard-8");
    }
}
