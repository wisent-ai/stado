//! The storage-location record family: [`ArtifactLocation`].

use serde_json::{Map, Value};

use super::{recorded_text, required_text};
use crate::artifacts_models::error::ArtifactError;

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
    /// A location names its `role` and `uri`; one missing either is not a
    /// location and is refused by the member it lacks. `storage`,
    /// `immutable_revision` and `sha256` are blank when the writer recorded
    /// none, and a count is a number or absent: one in another shape is
    /// refused by name rather than read as absent.
    pub(in crate::artifacts_models) fn from_value(value: &Value) -> Result<Self, ArtifactError> {
        let map = value
            .as_object()
            .ok_or_else(|| ArtifactError::invalid_manifest("location must be an object"))?;
        let count = |key: &str| -> Result<Option<i64>, ArtifactError> {
            match map.get(key) {
                None | Some(Value::Null) => Ok(None),
                Some(Value::Number(n)) => match n.as_i64() {
                    Some(count) => Ok(Some(count)),
                    None => Err(ArtifactError::invalid_manifest(format!(
                        "location {key} {n} is not a whole number"
                    ))),
                },
                Some(other) => Err(ArtifactError::invalid_manifest(format!(
                    "location {key} must be a number, found {other}"
                ))),
            }
        };
        Ok(Self {
            role: required_text("location", map, "role")?,
            uri: required_text("location", map, "uri")?,
            storage: recorded_text("location", map, "storage")?,
            immutable_revision: recorded_text("location", map, "immutable_revision")?,
            sha256: recorded_text("location", map, "sha256")?,
            size_bytes: count("size_bytes")?,
            file_count: count("file_count")?,
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
        // Serde writes an absent count as `null`.
        map.insert("size_bytes".into(), serde_json::json!(self.size_bytes));
        map.insert("file_count".into(), serde_json::json!(self.file_count));
        Value::Object(map)
    }
}
