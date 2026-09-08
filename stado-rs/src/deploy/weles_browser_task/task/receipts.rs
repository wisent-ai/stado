//! The receipt: everything one completed task reports back, and the report
//! object a caller prints it as.

use serde_json::{json, Map, Value};

/// Everything one completed task reports back.
pub struct TaskOutcome {
    pub run_id: String,
    pub ok: bool,
    pub exit_code: Option<i64>,
    pub result: Value,
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
        if let Some(profile) = &self.profile {
            object.insert("profile".to_string(), profile.clone());
        }
        object
    }
}
