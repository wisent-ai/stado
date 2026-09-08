//! The storage-location record family: [`ArtifactLocation`].

use serde_json::{Map, Value};

use crate::artifacts_models::error::{py_str, ArtifactError};

/// Where the artifact bytes live.
#[derive(Debug, Clone, PartialEq)]
pub struct ArtifactLocation {
    pub role: String,
    pub uri: String,
    pub storage: String,
    pub immutable_revision: String,
    pub sha256: String,
    pub size_bytes: Option<i64>,
    pub file_count: Option<i64>,
}

impl ArtifactLocation {
    /// Tolerant dict read: only known keys are picked up, missing optional
    /// fields take the dataclass defaults (Python `from_dict`).
    pub(in crate::artifacts_models) fn from_value(value: &Value) -> Result<Self, ArtifactError> {
        let map = value
            .as_object()
            .ok_or_else(|| ArtifactError::invalid_manifest("location must be an object"))?;
        let get_str = |key: &str, default: &str| {
            map.get(key)
                .map(py_str)
                .unwrap_or_else(|| default.to_string())
        };
        let get_opt_int = |key: &str| match map.get(key) {
            None | Some(Value::Null) => None,
            Some(Value::Number(n)) => n.as_i64(),
            // Python would crash on a non-int here; keep it tolerant.
            Some(_) => None,
        };
        Ok(Self {
            role: get_str("role", ""),
            uri: get_str("uri", ""),
            storage: get_str("storage", ""),
            immutable_revision: get_str("immutable_revision", ""),
            sha256: get_str("sha256", ""),
            size_bytes: get_opt_int("size_bytes"),
            file_count: get_opt_int("file_count"),
        })
    }

    pub(in crate::artifacts_models) fn to_value(&self) -> Value {
        let mut map = Map::new();
        map.insert("role".into(), Value::String(self.role.clone()));
        map.insert("uri".into(), Value::String(self.uri.clone()));
        map.insert("storage".into(), Value::String(self.storage.clone()));
        map.insert(
            "immutable_revision".into(),
            Value::String(self.immutable_revision.clone()),
        );
        map.insert("sha256".into(), Value::String(self.sha256.clone()));
        map.insert(
            "size_bytes".into(),
            self.size_bytes
                .map_or(Value::Null, |n| Value::Number(n.into())),
        );
        map.insert(
            "file_count".into(),
            self.file_count
                .map_or(Value::Null, |n| Value::Number(n.into())),
        );
        Value::Object(map)
    }
}
