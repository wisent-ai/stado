//! Job submission through compute.wisent.com or direct queue storage.
//!
//! The compute API key and repository/provider tokens are resolved from
//! Skarbiec. The API path posts to `{COMPUTE_API}/api/v1/instances`; the queue
//! path renders the startup script, writes it to internal queue storage and
//! the provider-neutral object namespace, then writes the queued job record.
//!
//! The components are the submission phases this file already separated:
//! [`request`] validates one submission and derives the canonical request and
//! its identity, [`placement`] resolves the hardware and builds the planned
//! job, [`manifest`] owns the durable run manifest that persists the plan
//! (validation, v2 migration and per-entry claims), and [`batch`] drives the
//! submission and returns the accepted jobs. The vocabulary every phase shares
//! stays here: the error, the options, the canonical JSON digest, submission
//! provenance and the storage handle. Every name a caller outside this module
//! uses is re-exported here, so `crate::queue::submit::<item>` resolves
//! exactly as before.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

use crate::config;
use crate::models::JobSecretRef;
use crate::queue::storage::JobStorage;
use crate::queue::StorageError;

mod batch;
mod manifest;
mod placement;
mod request;

pub use batch::submit_batch;
pub use request::{
    is_canonical_job_id, stable_run_id, submission_input_digest, submission_job_key,
    submission_source_digest, validate_run_id,
};

pub(crate) use manifest::{migrate_v2_run_manifest, validate_stored_run_manifest};
pub(crate) use request::immutable_job_projection;

// Shared inside the submission tree only: each phase imports what it needs
// from `crate::queue::submit` instead of reaching into a sibling component.
pub(in crate::queue::submit) use manifest::{
    checkpoint_accepted, claim_entry, validate_run_manifest, EntryClaim, SubmissionContext,
};
pub(in crate::queue::submit) use placement::{build_planned_job, resolve_hardware};
pub(in crate::queue::submit) use request::{
    job_id_from_key, submission_request, validate_recovered_job, validate_submission,
};

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

fn digest_value(value: &Value) -> String {
    hex::encode(Sha256::digest(json_dumps_sorted_compact(value).as_bytes()))
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
