//! The producing-run record family: [`ArtifactProducer`].

use serde_json::{Map, Value};

use crate::artifacts_models::error::{py_str, ArtifactError};

/// Provenance of the producing run.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ArtifactProducer {
    pub run_id: String,
    pub job_ids: Vec<String>,
    pub repo: String,
    pub commit: String,
    pub host: String,
}

impl ArtifactProducer {
    pub(in crate::artifacts_models) fn from_value(value: &Value) -> Result<Self, ArtifactError> {
        let map = value
            .as_object()
            .ok_or_else(|| ArtifactError::invalid_manifest("producer must be an object"))?;
        let get_str = |key: &str| map.get(key).map(py_str).unwrap_or_default();
        let job_ids = match map.get("job_ids") {
            None | Some(Value::Null) => Vec::new(),
            Some(Value::Array(items)) => items.iter().map(py_str).collect(),
            Some(other) => vec![py_str(other)],
        };
        Ok(Self {
            run_id: get_str("run_id"),
            job_ids,
            repo: get_str("repo"),
            commit: get_str("commit"),
            host: get_str("host"),
        })
    }

    pub(in crate::artifacts_models) fn to_value(&self) -> Value {
        let mut map = Map::new();
        map.insert("run_id".into(), Value::String(self.run_id.clone()));
        map.insert(
            "job_ids".into(),
            Value::Array(self.job_ids.iter().cloned().map(Value::String).collect()),
        );
        map.insert("repo".into(), Value::String(self.repo.clone()));
        map.insert("commit".into(), Value::String(self.commit.clone()));
        map.insert("host".into(), Value::String(self.host.clone()));
        Value::Object(map)
    }
}
