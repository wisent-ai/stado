//! The schedule record itself (Python `schedules/model.py`): the recurring job
//! spec, its durable occurrence reservation, the dataclass defaults, and the
//! JSON codec every persistence path reads and writes through.

use std::collections::BTreeMap;

use chrono::Utc;
use serde::{Deserialize, Serialize};

use crate::models::JobSecretRef;
use crate::queue::submit::SubmitOptions;

// ---------------------------------------------------------------------------
// Schedule model (Python schedules/model.py)
// ---------------------------------------------------------------------------

fn default_enabled() -> bool {
    true
}
fn default_tz() -> String {
    "UTC".into()
}
fn default_provider() -> String {
    "gcp".into()
}
fn default_repo_extras() -> String {
    "train".into()
}
fn default_policy() -> String {
    "skip".into()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScheduleOccurrenceReservation {
    pub occurrence_key: String,
    pub occurrence_at: String,
    pub run_id: String,
    pub state: String,
    pub owner: String,
    pub lease_expires_at: String,
}

/// A recurring job spec. Field order matches the Python dataclass so the
/// serialized JSON is key-order identical; missing keys take the
/// Python dataclass defaults and unknown keys are ignored (`from_dict`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Schedule {
    #[serde(default)]
    pub schedule_id: String,
    #[serde(default)]
    pub cron: String,
    #[serde(default)]
    pub command: String,
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    /// Durable deletion tombstone; retained so deletion cannot erase a claimed
    /// occurrence between reservation and enqueue.
    #[serde(default)]
    pub deleted: bool,
    #[serde(default = "default_tz")]
    pub tz: String,
    // ---- frozen durable submission options ----
    #[serde(default = "default_provider")]
    pub provider: String,
    #[serde(default)]
    pub gpu_type: String,
    #[serde(default)]
    pub vram_gb: i64,
    #[serde(default)]
    pub machine_type: String,
    #[serde(default)]
    pub priority: i64,
    #[serde(default)]
    pub preemptible: bool,
    #[serde(default)]
    pub max_cost_per_hour_usd: f64,
    #[serde(default)]
    pub pin_to_provider: bool,
    #[serde(default)]
    pub pinned_host: String,
    #[serde(default)]
    pub repo: String,
    #[serde(default)]
    pub repo_ref: String,
    #[serde(default)]
    pub repo_workdir: String,
    #[serde(default = "default_repo_extras")]
    pub repo_extras: String,
    #[serde(default)]
    pub pre_command: String,
    #[serde(default)]
    pub apt_packages: Vec<String>,
    #[serde(default)]
    pub output_uri: String,
    #[serde(default)]
    pub verify_command: String,
    #[serde(default)]
    pub exclusive: bool,
    #[serde(default)]
    pub secret_env: BTreeMap<String, JobSecretRef>,
    // ---- firing bookkeeping ----
    /// Filled by [`Schedule::finalize_new`] when empty (Python
    /// `__post_init__`).
    #[serde(default)]
    pub created_at: String,
    #[serde(default)]
    pub created_by: String,
    /// ISO-8601 UTC; "" disables firing.
    #[serde(default)]
    pub next_due_at: String,
    #[serde(default)]
    pub last_fired_at: Option<String>,
    #[serde(default)]
    pub last_run_id: String,
    /// Most recent fire's job_id (overlap-skip check).
    #[serde(default)]
    pub last_job_id: String,
    #[serde(default)]
    pub fire_count: i64,
    /// Durable occurrence reservation. It is written in the same CAS that
    /// advances next_due_at, then cleared only after durable run acceptance.
    #[serde(default)]
    pub pending_occurrence: Option<ScheduleOccurrenceReservation>,
    /// skip: do not fire while last_job_id is still in queue/ or running/.
    /// allow: fire regardless of prior instance.
    #[serde(default = "default_policy")]
    pub overlap_policy: String,
    /// skip: a coordinator-downtime gap collapses to a single fire and
    ///   next_due_at jumps to the next future occurrence.
    /// each: not yet honored beyond skip — reserved (see fire docs).
    #[serde(default = "default_policy")]
    pub catchup_policy: String,
}

impl Schedule {
    /// Python `__post_init__`: stamp `created_at` when empty.
    pub fn finalize_new(&mut self) {
        if self.created_at.is_empty() {
            self.created_at = crate::models::isoformat_utc(Utc::now());
        }
    }

    /// `Schedule(...)` with Python dataclass defaults.
    pub fn new(
        schedule_id: impl Into<String>,
        cron: impl Into<String>,
        command: impl Into<String>,
    ) -> Self {
        let mut sched: Schedule =
            serde_json::from_value(serde_json::Value::Object(serde_json::Map::new()))
                .expect("all fields have serde defaults");
        sched.schedule_id = schedule_id.into();
        sched.cron = cron.into();
        sched.command = command.into();
        sched.finalize_new();
        sched
    }

    /// The frozen options bound into one occurrence's durable run manifest.
    pub fn submit_options(&self) -> SubmitOptions {
        SubmitOptions {
            provider: self.provider.clone(),
            gpu_type: self.gpu_type.clone(),
            vram_gb: self.vram_gb,
            machine_type: self.machine_type.clone(),
            priority: self.priority,
            preemptible: self.preemptible,
            max_cost_per_hour_usd: self.max_cost_per_hour_usd,
            pin_to_provider: self.pin_to_provider,
            pinned_host: self.pinned_host.clone(),
            repo: self.repo.clone(),
            repo_ref: self.repo_ref.clone(),
            repo_workdir: self.repo_workdir.clone(),
            repo_extras: self.repo_extras.clone(),
            pre_command: self.pre_command.clone(),
            apt_packages: self.apt_packages.clone(),
            output_uri: self.output_uri.clone(),
            verify_command: self.verify_command.clone(),
            exclusive: self.exclusive,
            secret_env: self.secret_env.clone(),
            ..Default::default()
        }
    }

    /// Byte-compatible with Python `json.dumps(asdict(schedule), indent=2)`
    /// (ensure_ascii=True).
    pub fn to_json(&self) -> String {
        let pretty =
            serde_json::to_string_pretty(self).expect("Schedule serialization is infallible");
        crate::models::ensure_ascii(&pretty)
    }

    /// Python `Schedule.from_json` / `from_dict`: unknown keys ignored,
    /// missing keys defaulted, `created_at` post-init applied.
    pub fn from_json(s: &str) -> Result<Self, serde_json::Error> {
        let mut sched: Self = serde_json::from_str(s)?;
        sched.finalize_new();
        Ok(sched)
    }
}
