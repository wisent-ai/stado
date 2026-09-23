//! Immutable artifact registry domain models.
//!
//! Port of `stado/artifacts/models.py` ONLY — the frozen dataclasses and the
//! canonical-JSON serialization. The registry/validation/adapter logic
//! ported from `registry.py`, `validation.py` and `adapters/` lives in
//! [`crate::artifacts`].
//!
//! Canonical JSON is byte-compatible with Python
//! `json.dumps(obj, sort_keys=True, separators=(",", ":"))` (including
//! `ensure_ascii=True` escaping of every char >= 0x7f as \uXXXX), and
//! [`ArtifactManifest::manifest_sha256`] is the SHA-256 of that byte string.

use std::collections::BTreeMap;
use std::fmt;

use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

/// Pattern every ref segment must satisfy (Python `_SEGMENT`,
/// `^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$`), hand-rolled to avoid a regex
/// dependency: 1-128 chars, first ASCII alphanumeric, rest also allowing
/// `.`, `_`, `-`.
const SEGMENT_PATTERN: &str = "^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$";

fn is_segment(value: &str) -> bool {
    let mut chars = value.chars();
    match chars.next() {
        Some(first) if first.is_ascii_alphanumeric() => {}
        _ => return false,
    }
    value.len() <= 128 && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

/// Stable, machine-readable artifact operation failure (Python
/// `ArtifactError`). `code` is the machine-readable half (`ARTIFACT_*`).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{message}")]
pub struct ArtifactError {
    pub code: String,
    pub message: String,
}

impl ArtifactError {
    pub fn new(code: &str, message: impl Into<String>) -> Self {
        Self {
            code: code.to_string(),
            message: message.into(),
        }
    }

    fn invalid_ref(message: impl Into<String>) -> Self {
        Self::new("ARTIFACT_INVALID_REF", message)
    }

    fn invalid_manifest(message: impl Into<String>) -> Self {
        Self::new("ARTIFACT_INVALID_MANIFEST", message)
    }
}

fn segment(value: &str, label: &str) -> Result<String, ArtifactError> {
    if !is_segment(value) {
        return Err(ArtifactError::invalid_ref(format!(
            "{label} must match '{SEGMENT_PATTERN}'"
        )));
    }
    Ok(value.to_string())
}

/// `str()` semantics for JSON scalars, used by the tolerant `from_dict`
/// paths (Python stringifies whatever `dict.get` returns).
fn py_str(value: &Value) -> String {
    match value {
        Value::Null => "None".to_string(),
        Value::Bool(b) => if *b { "True" } else { "False" }.to_string(),
        Value::Number(n) => n.to_string(),
        Value::String(s) => s.clone(),
        // Python would render list/dict reprs; that is pathological input
        // for these fields, so compact JSON is close enough.
        other => other.to_string(),
    }
}

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
    fn from_value(value: &Value) -> Result<Self, ArtifactError> {
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
    fn from_value(value: &Value) -> Result<Self, ArtifactError> {
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

    fn to_value(&self) -> Value {
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
    fn from_value(value: &Value) -> Result<Self, ArtifactError> {
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

    fn to_value(&self) -> Value {
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
    fn from_value(value: &Value) -> Result<Self, ArtifactError> {
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

    fn to_value(&self) -> Value {
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

/// Result of one verification adapter run.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct VerificationReport {
    pub adapter: String,
    pub passed: bool,
    #[serde(default)]
    pub issues: Vec<String>,
    #[serde(default)]
    pub summary: Map<String, Value>,
}

/// Rebuild a JSON value with every object's keys in sorted order (the
/// crate enables serde_json's `preserve_order`, so insertion order is
/// serialization order).
fn canonicalize(value: &Value) -> Value {
    match value {
        Value::Object(map) => {
            let sorted: BTreeMap<&String, &Value> = map.iter().collect();
            let mut out = Map::with_capacity(map.len());
            for (key, item) in sorted {
                out.insert(key.clone(), canonicalize(item));
            }
            Value::Object(out)
        }
        Value::Array(items) => Value::Array(items.iter().map(canonicalize).collect()),
        other => other.clone(),
    }
}

/// Replicates Python's `ensure_ascii=True`: escapes every char >= 0x7f as
/// \uXXXX (with surrogate pairs for astral planes). Same implementation as
/// `models::ensure_ascii`.
fn ensure_ascii(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        if (ch as u32) < 0x7f {
            out.push(ch);
        } else {
            let mut buf = [0u16; 2];
            for unit in ch.encode_utf16(&mut buf) {
                out.push_str(&format!("\\u{:04x}", unit));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The exact input document used to generate the Python golden output
    /// below (via `stado/artifacts/models.py` running on Python 3.10).
    const GOLDEN_INPUT: &str = r##"{
        "type": "dataset",
        "namespace": "wisent",
        "name": "demo-artifact",
        "version": "v1",
        "title": "Démo artifact",
        "description": "round-trip żółć test",
        "created_at": "2026-05-01T00:00:00Z",
        "created_by": "tester",
        "producer": {"run_id": "run-1", "job_ids": ["j1", "j2"], "repo": "wisent", "commit": "abc123", "host": "box"},
        "locations": [
            {"role": "primary", "uri": "gs://stado/artifacts/demo", "storage": "gcs", "sha256": "deadbeef", "size_bytes": 4096, "file_count": 3},
            {"role": "mirror", "uri": "s3://bucket/demo", "storage": "s3"}
        ],
        "schemas": [{"kind": "jsonl", "fields": {"text": "string"}}],
        "summary": {"rows": 5, "score": 0.5, "note": "żółć"},
        "partitions": {"train": {"rows": 4}},
        "dependencies": ["model/wisent/base@v0", {"type": "dataset", "namespace": "wisent", "name": "parent", "version": "v2"}],
        "labels": {"tier": "gold", "count": 5, "active": true},
        "verification": {"adapter": "generic-v1", "verified_at": "2026-05-02T00:00:00Z", "result": "passed", "manifest_sha256": "cafe", "issues": ["minor"]},
        "schema_version": 1
    }"##;

    /// Python `ArtifactManifest.from_dict(doc).to_json()` on GOLDEN_INPUT.
    const GOLDEN_CANONICAL: &str = r#"{"created_at":"2026-05-01T00:00:00Z","created_by":"tester","dependencies":["model/wisent/base@v0","dataset/wisent/parent@v2"],"description":"round-trip \u017c\u00f3\u0142\u0107 test","labels":{"active":"True","count":"5","tier":"gold"},"locations":[{"file_count":3,"immutable_revision":"","role":"primary","sha256":"deadbeef","size_bytes":4096,"storage":"gcs","uri":"gs://stado/artifacts/demo"},{"file_count":null,"immutable_revision":"","role":"mirror","sha256":"","size_bytes":null,"storage":"s3","uri":"s3://bucket/demo"}],"partitions":{"train":{"rows":4}},"producer":{"commit":"abc123","host":"box","job_ids":["j1","j2"],"repo":"wisent","run_id":"run-1"},"ref":"dataset/wisent/demo-artifact@v1","schema_version":1,"schemas":[{"fields":{"text":"string"},"kind":"jsonl"}],"summary":{"note":"\u017c\u00f3\u0142\u0107","rows":5,"score":0.5},"title":"D\u00e9mo artifact","verification":{"adapter":"generic-v1","issues":["minor"],"manifest_sha256":"cafe","result":"passed","verified_at":"2026-05-02T00:00:00Z"}}"#;

    /// Python hashlib.sha256(canonical.encode()).hexdigest().
    const GOLDEN_SHA256: &str = "74bb9026b6b08912bca044389f3d8447b414345c1d2342b448b2daf96851d16e";

    #[test]
    fn ref_parse_and_format() {
        let r = ArtifactRef::parse("dataset/wisent/demo-artifact@v1").unwrap();
        assert_eq!(r.r#type, "dataset");
        assert_eq!(r.namespace, "wisent");
        assert_eq!(r.name, "demo-artifact");
        assert_eq!(r.version, "v1");
        assert_eq!(r.coordinate(), "dataset/wisent/demo-artifact");
        assert_eq!(r.to_string(), "dataset/wisent/demo-artifact@v1");
        assert_eq!(
            r.with_version("v2").unwrap().to_string(),
            "dataset/wisent/demo-artifact@v2"
        );
    }

    #[test]
    fn ref_rejects_malformed() {
        for bad in [
            "no-version",
            "a/b@v1",
            "a/b/c/d/e@v1",
            "/b/c@v1",
            "a//c@v1",
            "a b/c/d@v1",
        ] {
            let err = ArtifactRef::parse(bad).unwrap_err();
            assert_eq!(err.code, "ARTIFACT_INVALID_REF", "input: {bad}");
        }
        let err = ArtifactRef::parse("a/b/c@v 1").unwrap_err();
        assert_eq!(err.code, "ARTIFACT_INVALID_REF");
    }

    #[test]
    fn canonical_json_matches_python_byte_for_byte() {
        let manifest = ArtifactManifest::from_json(GOLDEN_INPUT).unwrap();
        assert_eq!(manifest.to_json(), GOLDEN_CANONICAL);
        assert_eq!(manifest.manifest_sha256(), GOLDEN_SHA256);
    }

    #[test]
    fn from_dict_defaults_and_tolerances() {
        let manifest = ArtifactManifest::from_json(GOLDEN_INPUT).unwrap();
        // Identity fallback fields moved into ref.
        assert_eq!(manifest.ref_.to_string(), "dataset/wisent/demo-artifact@v1");
        // Python str()-stringifies label values.
        assert_eq!(manifest.labels["count"], "5");
        assert_eq!(manifest.labels["active"], "True");
        // Missing optional location fields take dataclass defaults.
        assert_eq!(manifest.locations[1].immutable_revision, "");
        assert_eq!(manifest.locations[1].size_bytes, None);
        assert_eq!(manifest.locations[1].file_count, None);
        // Dict-form dependency parsed.
        assert_eq!(
            manifest.dependencies[1].to_string(),
            "dataset/wisent/parent@v2"
        );

        // Falsy title falls back to the artifact name.
        let m2 = ArtifactManifest::from_json(r#"{"ref": "a/b/c@v1", "title": ""}"#).unwrap();
        assert_eq!(m2.title, "c");
        assert_eq!(m2.verification.adapter, "generic-v1");
        assert_eq!(m2.schema_version, 1);
    }

    #[test]
    fn from_dict_rejects_bad_documents() {
        let err = ArtifactManifest::from_json("[1, 2]").unwrap_err();
        assert_eq!(err.code, "ARTIFACT_INVALID_MANIFEST");
        assert_eq!(err.message, "manifest must be an object");
        let err = ArtifactManifest::from_json(r#"{"namespace": "b"}"#).unwrap_err();
        assert_eq!(err.code, "ARTIFACT_INVALID_MANIFEST");
        assert_eq!(err.message, "missing manifest identity field: type");
        let err = ArtifactManifest::from_json("{invalid").unwrap_err();
        assert_eq!(err.code, "ARTIFACT_INVALID_MANIFEST");
        assert!(err.message.starts_with("invalid JSON:"));
    }
}
