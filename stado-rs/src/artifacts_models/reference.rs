//! The artifact coordinate record family: [`ArtifactRef`].

use std::fmt;

use serde_json::Value;

use super::error::{segment, ArtifactError};

/// Artifact coordinate: `<type>/<namespace>/<name>@<version>`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ArtifactRef {
    pub r#type: String,
    pub namespace: String,
    pub name: String,
    pub version: String,
}

impl ArtifactRef {
    /// Python `__post_init__`: every segment is validated on construction.
    pub fn new(
        r#type: &str,
        namespace: &str,
        name: &str,
        version: &str,
    ) -> Result<Self, ArtifactError> {
        Ok(Self {
            r#type: segment(r#type, "type")?,
            namespace: segment(namespace, "namespace")?,
            name: segment(name, "name")?,
            version: segment(version, "version")?,
        })
    }

    /// Parse `<type>/<namespace>/<name>@<version>`.
    pub fn parse(value: &str) -> Result<Self, ArtifactError> {
        let malformed = || {
            ArtifactError::invalid_ref("artifact ref must be <type>/<namespace>/<name>@<version>")
        };
        let (path, version) = value.rsplit_once('@').ok_or_else(malformed)?;
        let mut parts = path.splitn(3, '/');
        let (r#type, namespace, name) = match (parts.next(), parts.next(), parts.next()) {
            (Some(t), Some(ns), Some(n)) => (t, ns, n),
            _ => return Err(malformed()),
        };
        Self::new(r#type, namespace, name, version)
    }

    pub fn with_version(&self, version: &str) -> Result<Self, ArtifactError> {
        Self::new(&self.r#type, &self.namespace, &self.name, version)
    }

    pub fn coordinate(&self) -> String {
        format!("{}/{}/{}", self.r#type, self.namespace, self.name)
    }

    /// Deserialize either the string form or a dict with the four fields
    /// (Python accepts both in `ArtifactManifest.from_dict`).
    pub(super) fn from_value(value: &Value) -> Result<Self, ArtifactError> {
        match value {
            Value::String(s) => Self::parse(s),
            Value::Object(map) => Self::new(
                map.get("type").and_then(Value::as_str).unwrap_or(""),
                map.get("namespace").and_then(Value::as_str).unwrap_or(""),
                map.get("name").and_then(Value::as_str).unwrap_or(""),
                map.get("version").and_then(Value::as_str).unwrap_or(""),
            ),
            _ => Err(ArtifactError::invalid_ref(
                "artifact ref must be <type>/<namespace>/<name>@<version>",
            )),
        }
    }
}

impl fmt::Display for ArtifactRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}@{}", self.coordinate(), self.version)
    }
}
