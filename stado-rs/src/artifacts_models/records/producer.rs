//! The producing-run record family: [`ArtifactProducer`].

use serde_json::{Map, Value};

use super::{recorded_text, text_list};
use crate::artifacts_models::error::ArtifactError;

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
        // Every provenance field is optional in a manifest; one the manifest
        // omits is recorded empty, which `to_value` writes back as such. One
        // it writes in another type is refused by name.
        Ok(Self {
            run_id: recorded_text("producer", map, "run_id")?,
            job_ids: text_list("producer", map, "job_ids")?,
            repo: recorded_text("producer", map, "repo")?,
            commit: recorded_text("producer", map, "commit")?,
            host: recorded_text("producer", map, "host")?,
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
