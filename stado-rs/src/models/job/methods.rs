//! Behaviour of the job record: the `__post_init__` stamp, the defaulted
//! constructor, the JSON round-trip and the `Default` impl.

use serde_json::{Map, Value};

use super::record::Job;
use crate::models::python_compat::ensure_ascii;

impl Job {
    /// Why a job that just lost its worker for `reason` must fail instead of
    /// going back to the queue, or `None` when it goes back.
    ///
    /// No count decides it. A submitter that starts its own launches ends the
    /// job at its first loss. Otherwise the job is put back and the reason of
    /// that loss is kept in `error`; a second loss of the same kind (the
    /// reason's words before any parenthesised detail, so `VM reaped (dead
    /// agent, age=…)` and `VM reaped (wedged agent)` are one kind) says the
    /// loss follows the job, not the host, and ends it.
    pub fn restart_refusal(&self, reason: &str) -> Option<String> {
        if self.submitter_restarts {
            return Some(format!(
                "{reason}; its submitter starts every new launch itself"
            ));
        }
        let kind = |text: &str| match text.split_once('(') {
            Some((head, _detail)) => head.trim().to_string(),
            None => text.trim().to_string(),
        };
        self.error
            .as_deref()
            .filter(|previous| kind(previous) == kind(reason))
            .map(|previous| {
                format!("{reason}, after the same loss had already put it back once ({previous})")
            })
    }

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
