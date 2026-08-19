//! Job data model and state definitions.
//!
//! Port of `stado/models.py`. The JSON representation is byte-compatible with
//! the Python `Job.to_json()` (`json.dumps(asdict(job), indent=2)` with
//! `ensure_ascii=True`), including field declaration order. `from_dict`
//! tolerance maps to serde: unknown keys are ignored, missing keys fall back
//! to the Python dataclass defaults via `#[serde(default = ...)]`.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// Job lifecycle states. Serialized as plain strings; the state is also
/// redundantly encoded in the blob prefix (see `queue::storage`).
pub mod job_state {
    pub const QUEUED: &str = "queued";
    /// COMPLETED = extraction finished + handed off to the detached upload
    /// worker; NOT yet confirmed on HF. Kept named "completed" so the
    /// coordinator and dashboard stay unchanged.
    pub const COMPLETED: &str = "completed";
    /// UPLOADED = the upload worker confirmed the dir landed on HF (terminal).
    pub const UPLOADED: &str = "uploaded";
    pub const RUNNING: &str = "running";
    pub const FAILED: &str = "failed";
    pub const CANCELLED: &str = "cancelled";

    pub const ALL: [&str; 6] = [QUEUED, RUNNING, COMPLETED, UPLOADED, FAILED, CANCELLED];

    pub fn is_terminal(state: &str) -> bool {
        matches!(state, COMPLETED | UPLOADED | FAILED | CANCELLED)
    }
}

pub const DEPRECATED_ACTIVATION_ENTRYPOINT: &str = "wisent.scripts.activations.extract_and_upload";

pub fn deprecated_activation_command_reason(command: &str) -> &'static str {
    if !command.contains(DEPRECATED_ACTIVATION_ENTRYPOINT) {
        return "";
    }
    "refusing deprecated foreground activation uploader; use \
     wisent.scripts.activations.raw.extract_and_upload so extraction \
     hands upload to the detached worker pool"
}

/// Activation extraction jobs are VRAM-sized, not whole-GPU-exclusive.
pub fn activation_extraction_must_share_gpu(command: &str) -> bool {
    command.contains("wisent.scripts.activations.raw.extract_and_upload")
}

fn default_provider() -> String {
    "gcp".into()
}
fn default_state() -> String {
    job_state::QUEUED.into()
}
fn default_max_restarts() -> i64 {
    20
}
fn default_image() -> String {
    "pytorch-2-9-cu129-ubuntu-2204-nvidia-580-v20260408".into()
}
fn default_image_project() -> String {
    "deeplearning-platform-release".into()
}
fn default_boot_disk_gb() -> i64 {
    500
}
fn default_max_preempts() -> i64 {
    3
}
fn default_repo_extras() -> String {
    "train".into()
}
fn default_yield_grace() -> i64 {
    120
}
fn default_max_yields() -> i64 {
    5
}
fn default_executor() -> String {
    "stado-agent".into()
}

/// A named workload secret resolved by the agent immediately before spawn.
/// Queue records contain only this reference; plaintext never enters storage.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobSecretRef {
    pub item: String,
    pub field: String,
}

/// The central job record. Field order matches the Python dataclass so
/// serialized JSON is key-order identical.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Job {
    #[serde(default)]
    pub job_id: String,
    #[serde(default)]
    pub command: String,
    #[serde(default)]
    pub gpu_mem_gb: i64,
    #[serde(default)]
    pub gpu_type: String,
    #[serde(default)]
    pub machine_type: String,
    #[serde(default = "default_provider")]
    pub provider: String,
    #[serde(default)]
    pub batch_id: String,
    #[serde(default = "default_state")]
    pub state: String,
    /// ISO-8601 UTC; filled by [`Job::finalize_new`] when empty (Python
    /// `__post_init__`).
    #[serde(default)]
    pub created_at: String,
    #[serde(default)]
    pub started_at: Option<String>,
    #[serde(default)]
    pub completed_at: Option<String>,
    #[serde(default)]
    pub failed_at: Option<String>,
    #[serde(default)]
    pub instance_ref: Option<String>,
    #[serde(default)]
    pub restarts: i64,
    #[serde(default = "default_max_restarts")]
    pub max_restarts: i64,
    #[serde(default)]
    pub last_restart: Option<String>,
    #[serde(default = "default_image")]
    pub image: String,
    #[serde(default = "default_image_project")]
    pub image_project: String,
    #[serde(default = "default_boot_disk_gb")]
    pub boot_disk_gb: i64,
    #[serde(default)]
    pub startup_script_uri: String,
    #[serde(default)]
    pub error: Option<String>,
    /// If true, dispatch on Spot.
    #[serde(default)]
    pub preemptible: bool,
    /// If true, only the named provider claims.
    #[serde(default)]
    pub pin_to_provider: bool,
    /// 0 = no cap.
    #[serde(default)]
    pub max_cost_per_hour_usd: f64,
    /// # times this job was preempted on Spot.
    #[serde(default)]
    pub preempt_count: i64,
    /// After N preempts, fall back to on-demand.
    #[serde(default = "default_max_preempts")]
    pub max_preempts_before_ondemand: i64,
    /// Higher = scheduled first within FIFO bucket.
    #[serde(default)]
    pub priority: i64,
    /// Failed create_instance calls; backs off dispatch. Resets on success.
    #[serde(default)]
    pub dispatch_attempts: i64,
    #[serde(default)]
    pub last_dispatch_attempt: Option<String>,
    // Submitter provenance ($USER + hostname at submit time).
    #[serde(default)]
    pub submitted_by: String,
    #[serde(default)]
    pub submitted_from: String,
    /// cli | api | other
    #[serde(default)]
    pub submitted_via: String,
    /// One `stado submit` invocation = one run (runs/<run_id>.json).
    #[serde(default)]
    pub run_id: String,
    /// Orchestrator name from $WC_SUBMITTER_APP.
    #[serde(default)]
    pub submitter_app: String,
    // Optional source repo to git clone before running command.
    #[serde(default)]
    pub repo: String,
    /// Exact source commit. Repository workloads must pin a full lowercase SHA-1.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub repo_ref: String,
    #[serde(default)]
    pub repo_workdir: String,
    /// pip-install extras name; "" to skip install.
    #[serde(default = "default_repo_extras")]
    pub repo_extras: String,
    /// Post-exit-0 verification hook; non-zero reverses COMPLETED -> FAILED.
    #[serde(default)]
    pub verify_command: String,
    /// Coordinator assignment hint (rewritten every tick).
    #[serde(default)]
    pub assigned_to: String,
    /// Operator hard-pin to one consumer_id; the makespan matcher never
    /// touches it.
    #[serde(default)]
    pub pinned_host: String,
    /// Runtime hint (seconds); 0 = fall back to historical means.
    #[serde(default)]
    pub runtime_seconds_estimate: f64,
    /// Shell snippet prefixed to job.command at agent runtime.
    #[serde(default)]
    pub pre_command: String,
    /// Apt packages (cloud-kind agents only).
    #[serde(default)]
    pub apt_packages: Vec<String>,
    /// Additive output mirror URI (default location always written).
    #[serde(default)]
    pub output_uri: String,
    /// Exclusive GPU use: agent claims only with zero other active slots.
    #[serde(default)]
    pub exclusive: bool,
    /// Measured peak GPU memory (GiB); 0 = not measured.
    #[serde(default)]
    pub peak_vram_gb: i64,
    /// True iff peak_vram_gb came from the per-GPU probe (0.4.241+).
    #[serde(default)]
    pub peak_vram_per_gpu: bool,
    /// Set when submitted by a recurring schedule.
    #[serde(default)]
    pub schedule_id: String,
    /// Original failed job_id when this is a re-submission.
    #[serde(default)]
    pub re_submission_of: String,
    // Cooperative-yield (background) job contract.
    #[serde(default)]
    pub yieldable: bool,
    #[serde(default)]
    pub yield_command: String,
    #[serde(default = "default_yield_grace")]
    pub yield_grace_seconds: i64,
    #[serde(default)]
    pub yield_count: i64,
    #[serde(default = "default_max_yields")]
    pub max_yields_before_protected: i64,
    // Provider-neutral placement and execution requirements.
    #[serde(default = "default_executor")]
    pub executor: String,
    #[serde(default)]
    pub platform_os: String,
    #[serde(default)]
    pub architecture: String,
    #[serde(default)]
    pub cpu_cores: i64,
    #[serde(default)]
    pub memory_gb: i64,
    #[serde(default)]
    pub disk_gb: i64,
    #[serde(default)]
    pub region: String,
    // Structured provider execution (box-prompt etc.).
    #[serde(default)]
    pub box_ttl_seconds: i64,
    #[serde(default)]
    pub prompt: String,
    #[serde(default)]
    pub prompt_provider: String,
    #[serde(default)]
    pub prompt_model: String,
    #[serde(default)]
    pub prompt_reasoning_effort: String,
    /// Explicit per-job environment variables backed by scoped Skarbiec fields.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub secret_env: BTreeMap<String, JobSecretRef>,
    // Named, reproducible artifact inputs.
    #[serde(default)]
    pub input_artifacts: Map<String, Value>,
    #[serde(default)]
    pub resolved_input_artifacts: Map<String, Value>,
    #[serde(default)]
    pub artifact_paths: Vec<String>,
    /// Hard completion deadline for autonomous placement (RFC 3339 UTC).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deadline_at: Option<String>,
}

impl Job {
    /// Python `__post_init__`: stamp `created_at` when empty.
    pub fn finalize_new(&mut self) {
        if self.created_at.is_empty() {
            self.created_at = chrono::Utc::now().to_rfc3339();
        }
    }

    /// Python `Job.new(job_id=..., command=...)` equivalent with defaults.
    pub fn new(job_id: impl Into<String>, command: impl Into<String>) -> Self {
        let mut job: Job = serde_json::from_value(Value::Object(Map::new()))
            .expect("all fields have serde defaults");
        job.job_id = job_id.into();
        job.command = command.into();
        job.finalize_new();
        job
    }

    /// Byte-compatible with Python `json.dumps(asdict(job), indent=2)`
    /// (ensure_ascii=True: non-ASCII escaped as \uXXXX).
    pub fn to_json(&self) -> String {
        let pretty = serde_json::to_string_pretty(self).expect("Job serialization is infallible");
        ensure_ascii(&pretty)
    }

    /// Python `Job.from_json` / `from_dict`: unknown keys ignored, missing
    /// keys defaulted.
    pub fn from_json(s: &str) -> Result<Self, serde_json::Error> {
        let mut job: Self = serde_json::from_str(s)?;
        job.finalize_new();
        Ok(job)
    }
}

impl Default for Job {
    fn default() -> Self {
        Self::new("", "")
    }
}

/// Python `datetime.isoformat()` for a UTC datetime: `+00:00` suffix, with
/// 6-digit microseconds only when nonzero (Python omits the fraction when
/// `microsecond == 0`).
pub(crate) fn isoformat_utc(dt: chrono::DateTime<chrono::Utc>) -> String {
    if dt.timestamp_subsec_micros() == 0 {
        dt.format("%Y-%m-%dT%H:%M:%S+00:00").to_string()
    } else {
        dt.format("%Y-%m-%dT%H:%M:%S%.6f+00:00").to_string()
    }
}

/// Replicates Python's `ensure_ascii=True`: escapes every non-ASCII char as
/// \uXXXX (with surrogate pairs for astral planes). Already-escaped sequences
/// and structural characters are untouched.
pub(crate) fn ensure_ascii(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        if (ch as u32) < 0x7f {
            out.push(ch);
        } else {
            let mut buf = [0u16; 2];
            for unit in ch.encode_utf16(&mut buf) {
                out.push_str(&format!("\\u{:04x}", unit));
            }
        }
    }
    out
}

/// Recursively sort object keys (Python `sort_keys=True`).
pub(crate) fn sort_keys(value: &Value) -> Value {
    match value {
        Value::Object(map) => {
            let btree: std::collections::BTreeMap<String, Value> =
                map.iter().map(|(k, v)| (k.clone(), sort_keys(v))).collect();
            Value::Object(btree.into_iter().collect())
        }
        Value::Array(items) => Value::Array(items.iter().map(sort_keys).collect()),
        other => other.clone(),
    }
}

/// Python `json.dumps(value, indent=2, sort_keys=True)` (ensure_ascii=True).
pub(crate) fn json_dumps_pretty_sorted(value: &Value) -> String {
    let pretty =
        serde_json::to_string_pretty(&sort_keys(value)).expect("JSON serialization is infallible");
    ensure_ascii(&pretty)
}

/// Python `repr()` of a string: single quotes by default, double quotes when
/// the string contains a single quote (and no double quote); backslash-escapes
/// for the quote, backslash, and the usual control characters.
pub(crate) fn py_str_repr(s: &str) -> String {
    let quote = if s.contains('\'') && !s.contains('"') {
        '"'
    } else {
        '\''
    };
    let mut out = String::with_capacity(s.len() + 2);
    out.push(quote);
    for ch in s.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c == quote => {
                out.push('\\');
                out.push(c);
            }
            c if (c as u32) < 0x20 || (c as u32) == 0x7f => {
                out.push_str(&format!("\\x{:02x}", c as u32));
            }
            c => out.push(c),
        }
    }
    out.push(quote);
    out
}

