//! The stored-verification record family: [`ArtifactVerification`].

use serde_json::{Map, Value};

use super::{as_text, required_text, text_list};
use crate::artifacts_models::error::ArtifactError;

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
        // A verification names when it ran, what it found and which
        // manifest it hashed; one missing any of them is not a verification
        // and is refused by the member it lacks. The adapter alone has a
        // declared default, the one `Default` names.
        Ok(Self {
            adapter: match map.get("adapter") {
                None | Some(Value::Null) => Self::default().adapter,
                Some(adapter) => as_text("verification", "adapter", adapter)?,
            },
            verified_at: required_text("verification", map, "verified_at")?,
            result: required_text("verification", map, "result")?,
            manifest_sha256: required_text("verification", map, "manifest_sha256")?,
            issues: text_list("verification", map, "issues")?,
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
