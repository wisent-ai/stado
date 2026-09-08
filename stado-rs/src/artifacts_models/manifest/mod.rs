//! The immutable manifest document record family: [`ArtifactManifest`].

mod canonical;

use std::collections::BTreeMap;

use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

use super::error::{py_str, ArtifactError};
use super::records::{ArtifactLocation, ArtifactProducer, ArtifactVerification};
use super::reference::ArtifactRef;
use canonical::{canonicalize, ensure_ascii};

/// The immutable manifest document.
#[derive(Debug, Clone, PartialEq)]
pub struct ArtifactManifest {
    /// Python field name is `ref`.
    pub ref_: ArtifactRef,
    pub title: String,
    pub description: String,
    pub created_at: String,
    pub created_by: String,
    pub producer: ArtifactProducer,
    pub locations: Vec<ArtifactLocation>,
    pub schemas: Vec<Map<String, Value>>,
    pub summary: Map<String, Value>,
    pub partitions: Map<String, Value>,
    pub dependencies: Vec<ArtifactRef>,
    pub labels: BTreeMap<String, String>,
    pub verification: ArtifactVerification,
    pub schema_version: i64,
}

impl ArtifactManifest {
    pub fn new(ref_: ArtifactRef, title: impl Into<String>) -> Self {
        Self {
            ref_,
            title: title.into(),
            description: String::new(),
            created_at: String::new(),
            created_by: String::new(),
            producer: ArtifactProducer::default(),
            locations: Vec::new(),
            schemas: Vec::new(),
            summary: Map::new(),
            partitions: Map::new(),
            dependencies: Vec::new(),
            labels: BTreeMap::new(),
            verification: ArtifactVerification::default(),
            schema_version: 1,
        }
    }

    /// Tolerant dict read (Python `from_dict`). Identity comes from a `ref`
    /// member (string or dict) or top-level type/namespace/name/version.
    pub fn from_dict(value: &Value) -> Result<Self, ArtifactError> {
        let map = value
            .as_object()
            .ok_or_else(|| ArtifactError::invalid_manifest("manifest must be an object"))?;

        let ref_ = match map.get("ref") {
            Some(ref_value @ (Value::String(_) | Value::Object(_))) => {
                ArtifactRef::from_value(ref_value)?
            }
            _ => {
                let identity_field = |key: &str| -> Result<&str, ArtifactError> {
                    map.get(key).and_then(Value::as_str).ok_or_else(|| {
                        ArtifactError::invalid_manifest(format!(
                            "missing manifest identity field: {key}"
                        ))
                    })
                };
                ArtifactRef::new(
                    identity_field("type")?,
                    identity_field("namespace")?,
                    identity_field("name")?,
                    identity_field("version")?,
                )?
            }
        };

        let locations = match map.get("locations") {
            None | Some(Value::Null) => Vec::new(),
            Some(Value::Array(items)) => items
                .iter()
                .map(ArtifactLocation::from_value)
                .collect::<Result<Vec<_>, _>>()?,
            Some(_) => {
                return Err(ArtifactError::invalid_manifest(
                    "locations must be an array",
                ))
            }
        };
        let producer = match map.get("producer") {
            None | Some(Value::Null) => ArtifactProducer::default(),
            Some(p) => ArtifactProducer::from_value(p)?,
        };
        let dependencies = match map.get("dependencies") {
            None | Some(Value::Null) => Vec::new(),
            Some(Value::Array(items)) => items
                .iter()
                .map(ArtifactRef::from_value)
                .collect::<Result<Vec<_>, _>>()?,
            Some(_) => {
                return Err(ArtifactError::invalid_manifest(
                    "dependencies must be an array",
                ))
            }
        };
        let verification = match map.get("verification") {
            None | Some(Value::Null) => ArtifactVerification::default(),
            Some(v) => ArtifactVerification::from_value(v)?,
        };
        let schemas = match map.get("schemas") {
            None | Some(Value::Null) => Vec::new(),
            Some(Value::Array(items)) => items
                .iter()
                .map(|item| {
                    item.as_object().cloned().ok_or_else(|| {
                        ArtifactError::invalid_manifest("schema entries must be objects")
                    })
                })
                .collect::<Result<Vec<_>, _>>()?,
            Some(_) => return Err(ArtifactError::invalid_manifest("schemas must be an array")),
        };
        let get_map = |key: &str| -> Result<Map<String, Value>, ArtifactError> {
            match map.get(key) {
                None | Some(Value::Null) => Ok(Map::new()),
                Some(Value::Object(m)) => Ok(m.clone()),
                Some(_) => Err(ArtifactError::invalid_manifest(format!(
                    "{key} must be an object"
                ))),
            }
        };
        let labels = match map.get("labels") {
            None | Some(Value::Null) => BTreeMap::new(),
            Some(Value::Object(m)) => m.iter().map(|(k, v)| (k.clone(), py_str(v))).collect(),
            Some(_) => return Err(ArtifactError::invalid_manifest("labels must be an object")),
        };
        let schema_version = match map.get("schema_version") {
            None | Some(Value::Null) => 1,
            Some(Value::Number(n)) => n.as_i64().ok_or_else(|| {
                ArtifactError::invalid_manifest("schema_version must be an integer")
            })?,
            Some(Value::String(s)) => s.trim().parse::<i64>().map_err(|_| {
                ArtifactError::invalid_manifest("schema_version must be an integer")
            })?,
            Some(_) => {
                return Err(ArtifactError::invalid_manifest(
                    "schema_version must be an integer",
                ))
            }
        };
        // Python: title=str(value.get("title") or ref.name) — a falsy title
        // falls back to the artifact name.
        let title = match map.get("title") {
            None | Some(Value::Null) => ref_.name.clone(),
            Some(Value::String(s)) if s.is_empty() => ref_.name.clone(),
            Some(other) => py_str(other),
        };
        Ok(Self {
            ref_,
            title,
            description: map.get("description").map(py_str).unwrap_or_default(),
            created_at: map.get("created_at").map(py_str).unwrap_or_default(),
            created_by: map.get("created_by").map(py_str).unwrap_or_default(),
            producer,
            locations,
            schemas,
            summary: get_map("summary")?,
            partitions: get_map("partitions")?,
            dependencies,
            labels,
            verification,
            schema_version,
        })
    }

    /// Python `from_json`: parse then `from_dict`.
    pub fn from_json(value: &str) -> Result<Self, ArtifactError> {
        let parsed: Value = serde_json::from_str(value)
            .map_err(|exc| ArtifactError::invalid_manifest(format!("invalid JSON: {exc}")))?;
        Self::from_dict(&parsed)
    }

    /// Python `to_dict`: ref and dependencies serialized in string form.
    pub fn to_dict(&self) -> Value {
        let mut map = Map::new();
        map.insert("ref".into(), Value::String(self.ref_.to_string()));
        map.insert("title".into(), Value::String(self.title.clone()));
        map.insert(
            "description".into(),
            Value::String(self.description.clone()),
        );
        map.insert("created_at".into(), Value::String(self.created_at.clone()));
        map.insert("created_by".into(), Value::String(self.created_by.clone()));
        map.insert("producer".into(), self.producer.to_value());
        map.insert(
            "locations".into(),
            Value::Array(
                self.locations
                    .iter()
                    .map(ArtifactLocation::to_value)
                    .collect(),
            ),
        );
        map.insert(
            "schemas".into(),
            Value::Array(self.schemas.iter().cloned().map(Value::Object).collect()),
        );
        map.insert("summary".into(), Value::Object(self.summary.clone()));
        map.insert("partitions".into(), Value::Object(self.partitions.clone()));
        map.insert(
            "dependencies".into(),
            Value::Array(
                self.dependencies
                    .iter()
                    .map(|r| Value::String(r.to_string()))
                    .collect(),
            ),
        );
        map.insert(
            "labels".into(),
            Value::Object(
                self.labels
                    .iter()
                    .map(|(k, v)| (k.clone(), Value::String(v.clone())))
                    .collect(),
            ),
        );
        map.insert("verification".into(), self.verification.to_value());
        map.insert(
            "schema_version".into(),
            Value::Number(self.schema_version.into()),
        );
        Value::Object(map)
    }

    /// Canonical JSON, byte-compatible with Python
    /// `json.dumps(to_dict(), sort_keys=True, separators=(",", ":"))`.
    pub fn to_json(&self) -> String {
        let canonical = canonicalize(&self.to_dict());
        let compact =
            serde_json::to_string(&canonical).expect("manifest serialization is infallible");
        ensure_ascii(&compact)
    }

    /// SHA-256 hex digest of the canonical JSON byte string.
    pub fn manifest_sha256(&self) -> String {
        let mut hasher = Sha256::new();
        hasher.update(self.to_json().as_bytes());
        hex::encode(hasher.finalize())
    }
}
