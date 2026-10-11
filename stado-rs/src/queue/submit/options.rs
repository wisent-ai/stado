//! Every durable submission option, and the defaults a caller starts from.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::models::JobSecretRef;

use super::ResolvedHardwareProjection;

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
    /// The submitter starts every new launch itself, so Stado never puts the
    /// job back after a lost worker ([`crate::models::Job::restart_refusal`]).
    /// Written only when true: Stado agents built before the key existed read
    /// submission options with unknown keys refused, and a plan naming it
    /// left every job such an agent ran stuck in `running`, its transition
    /// to `failed` or `completed` refused as an unreadable run manifest.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub submitter_restarts: bool,
    /// Plans written while a job carried a restart count can name one. Stado
    /// keeps no count now, so the key is read and ignored, only so those
    /// stored plans still parse when a submission resumes them.
    #[serde(default, rename = "max_restarts", skip_serializing)]
    pub retired_max_restarts: Option<serde::de::IgnoredAny>,
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
    /// CPU cores the job needs on its worker. 0 leaves the worker's own
    /// default (one core); a placed workload declares the kind's reservation
    /// here so the host admits it against what is really free.
    #[serde(default)]
    pub cpu_cores: i64,
    /// RAM in GiB the job needs on its worker. 0 leaves the worker's own
    /// default (one GiB). See [`SubmitOptions::cpu_cores`].
    #[serde(default)]
    pub memory_gb: i64,
    pub pre_command: String,
    pub apt_packages: Vec<String>,
    pub output_uri: String,
    pub verify_command: String,
    pub exclusive: bool,
    /// See `Job::terminates_agent`.
    #[serde(default)]
    pub terminates_agent: bool,
    pub run_id: String,
    pub schedule_id: String,
    pub re_submission_of: String,
    pub yieldable: bool,
    pub yield_command: String,
    /// How often the job may be yielded before it is protected, stated with
    /// `--max-yields`; `None` protects it from the start. Written only when
    /// stated, so agents built before the key existed still read the plan.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_yields_before_protected: Option<i64>,
    /// The boot or container disk in GB a rented machine gets for this job,
    /// stated with `--boot-disk-gb`; `None` means no machine is rented for
    /// it. Written only when stated, as above.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub boot_disk_gb: Option<i64>,
    /// The run time in seconds the submitter states, stated with
    /// `--runtime-seconds-estimate`. Placement prices and orders a job by it
    /// until a completed run of its model on that hardware is measured; a
    /// job with neither is refused a priced placement by name. Written only
    /// when stated, as above.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime_seconds_estimate: Option<f64>,
    /// Read-only acceptance of a retired submission field. Existing queue
    /// records remain readable so they can run or be retired; new records
    /// do not serialize it and scheduling does not use its value.
    #[serde(default, rename = "yield_grace_seconds", skip_serializing)]
    pub retired_yield_grace_seconds: Option<u64>,
    pub pinned_host: String,
    pub secret_env: BTreeMap<String, JobSecretRef>,
    pub input_artifacts: Map<String, Value>,
    pub resolved_input_artifacts: Map<String, Value>,
}

impl Default for SubmitOptions {
    /// Stado defaults: no provider pin, no pip extras, everything else
    /// empty/zero/false.
    fn default() -> Self {
        Self {
            provider: String::new(),
            batch_id: String::new(),
            bucket: String::new(),
            preemptible: false,
            max_cost_per_hour_usd: 0.0,
            submitter_restarts: false,
            retired_max_restarts: None,
            pin_to_provider: false,
            priority: 0,
            deadline_at: None,
            repo: String::new(),
            repo_ref: String::new(),
            repo_workdir: String::new(),
            repo_extras: String::new(),
            gpu_type: String::new(),
            vram_gb: 0,
            machine_type: String::new(),
            resolved_hardware: None,
            platform_os: String::new(),
            architecture: String::new(),
            cpu_cores: 0,
            memory_gb: 0,
            pre_command: String::new(),
            apt_packages: vec![],
            output_uri: String::new(),
            verify_command: String::new(),
            exclusive: false,
            terminates_agent: false,
            run_id: String::new(),
            schedule_id: String::new(),
            re_submission_of: String::new(),
            yieldable: false,
            yield_command: String::new(),
            max_yields_before_protected: None,
            boot_disk_gb: None,
            runtime_seconds_estimate: None,
            retired_yield_grace_seconds: None,
            pinned_host: String::new(),
            secret_env: BTreeMap::new(),
            input_artifacts: Map::new(),
            resolved_input_artifacts: Map::new(),
        }
    }
}
