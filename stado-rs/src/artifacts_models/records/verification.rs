//! The stored-verification record family: [`ArtifactVerification`].

use serde_json::{Map, Value};

use crate::artifacts_models::error::{py_str, ArtifactError};

/// Outcome of the last verification pass.
#[derive(Debug, Clone, PartialEq)]
pub struct ArtifactVerification {
    pub adapter: String,
    pub verified_at: String,
    pub result: String,
    pub manifest_sha256: String,
    pub issues: Vec<String>,
}

impl Default for ArtifactVerification {
    fn default() -> Self {
        Self {
            adapter: "generic-v1".to_string(),
            verified_at: String::new(),
            result: String::new(),
            manifest_sha256: String::new(),
            issues: Vec::new(),
        }
    }
}

impl ArtifactVerification {
    pub(in crate::artifacts_models) fn from_value(value: &Value) -> Result<Self, ArtifactError> {
        let map = value
            .as_object()
            .ok_or_else(|| ArtifactError::invalid_manifest("verification must be an object"))?;
        let get_str = |key: &str| map.get(key).map(py_str).unwrap_or_default();
        let issues = match map.get("issues") {
            None | Some(Value::Null) => Vec::new(),
            Some(Value::Array(items)) => items.iter().map(py_str).collect(),
            Some(other) => vec![py_str(other)],
        };
        Ok(Self {
            adapter: map
                .get("adapter")
                .map(py_str)
                .unwrap_or_else(|| "generic-v1".into()),
            verified_at: get_str("verified_at"),
            result: get_str("result"),
            manifest_sha256: get_str("manifest_sha256"),
            issues,
        })
    }

    pub(in crate::artifacts_models) fn to_value(&self) -> Value {
        let mut map = Map::new();
        map.insert("adapter".into(), Value::String(self.adapter.clone()));
        map.insert(
            "verified_at".into(),
            Value::String(self.verified_at.clone()),
        );
        map.insert("result".into(), Value::String(self.result.clone()));
        map.insert(
            "manifest_sha256".into(),
            Value::String(self.manifest_sha256.clone()),
        );
        map.insert(
            "issues".into(),
            Value::Array(self.issues.iter().cloned().map(Value::String).collect()),
        );
        Value::Object(map)
    }
}
