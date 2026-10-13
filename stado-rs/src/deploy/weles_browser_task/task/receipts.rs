//! The receipt: everything one completed task reports back, and the report
//! object a caller prints it as.

use serde_json::{json, Map, Value};

/// Everything one completed task reports back.
pub struct TaskOutcome {
    pub run_id: String,
    pub ok: bool,
    pub exit_code: Option<i64>,
    /// The trajectory's own result: its last JSON line, or the result
    /// document it left; null when it left neither.
    pub result: Value,
    /// The named documents the run left in its recording tree, by the
    /// name Weles reads each under (`pangram`,
    /// `overleaf_version_history_summary`, `ban_signal`, …): what a caller
    /// reads when the trajectory writes a file rather than printing.
    pub outputs: Value,
    /// Present only when one of those documents could not be read: the
    /// file by name and the reader's error, so unreadable is told from
    /// absent.
    pub output_errors: Option<Value>,
    pub stdout_tail: String,
    pub stderr_tail: String,
    pub profile: Option<Value>,
}

impl TaskOutcome {
    pub fn to_report(&self, target: &str, action: &str) -> Map<String, Value> {
        let mut object = Map::new();
        object.insert("host".to_string(), json!(target));
        object.insert("action".to_string(), json!(action));
        object.insert("run_id".to_string(), json!(self.run_id));
        object.insert("ok".to_string(), json!(self.ok));
        object.insert("exit_code".to_string(), json!(self.exit_code));
        object.insert("result".to_string(), self.result.clone());
        object.insert("outputs".to_string(), self.outputs.clone());
        if let Some(errors) = &self.output_errors {
            object.insert("output_errors".to_string(), errors.clone());
        }
        object.insert("stdout_tail".to_string(), json!(self.stdout_tail));
        object.insert("stderr_tail".to_string(), json!(self.stderr_tail));
        if let Some(profile) = &self.profile {
            object.insert("profile".to_string(), profile.clone());
        }
        object
    }
}
