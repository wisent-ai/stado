//! Location/shard patterns and the typed reader of the activation-dataset
//! specification a manifest carries.

use regex::Regex;
use serde_json::{Map, Value};

pub(super) fn hf_location_re() -> &'static Regex {
    static RE: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"^hf://datasets/([^@]+)@([0-9a-fA-F]{40,64})$").expect("static regex compiles")
    })
}

pub(super) fn raw_shard_re() -> &'static Regex {
    static RE: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"/layer_\d+_chunk_\d+\.safetensors$").expect("static regex compiles")
    })
}

pub(super) fn aggregated_shard_re() -> &'static Regex {
    static RE: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    RE.get_or_init(|| Regex::new(r"/layer_\d+\.safetensors$").expect("static regex compiles"))
}

/// The partition key the specification lives under, and the prefix every
/// issue about one of its fields names.
const SPEC: &str = "activation_dataset";

/// The activation-dataset specification as the manifest states it. Every
/// field the manifest producer writes is required and typed: a missing or
/// mistyped one is named as an issue instead of being read as a default
/// (truthiness read `"false"` as true, an absent `require_complete_markers`
/// as true, a missing root as a guessed directory and a list that is not a
/// list as no list at all).
pub(super) struct DatasetSpec {
    pub(super) models: Vec<String>,
    pub(super) require_complete: bool,
    pub(super) raw: Partition,
    pub(super) aggregated: Partition,
}

/// One stored partition: the directory it lives under and the benchmarks
/// and prompt formats it holds a leaf for, per model.
pub(super) struct Partition {
    pub(super) root: String,
    pub(super) benchmarks: Vec<String>,
    pub(super) formats: Vec<String>,
}

impl DatasetSpec {
    /// The specification, or every issue that keeps it from being one.
    pub(super) fn read(spec: &Map<String, Value>) -> Result<Self, Vec<String>> {
        let mut issues = Vec::new();
        let models = kept(text_list(spec, "models", SPEC), &mut issues);
        if models.as_ref().is_some_and(Vec::is_empty) {
            issues.push(format!("{SPEC}.models must be a non-empty list"));
        }
        let require_complete = kept(flag(spec, "require_complete_markers", SPEC), &mut issues);
        let raw = Partition::read(spec, "raw", &mut issues);
        let aggregated = Partition::read(spec, "aggregated", &mut issues);
        match (models, require_complete, raw, aggregated) {
            (Some(models), Some(require_complete), Some(raw), Some(aggregated))
                if issues.is_empty() =>
            {
                Ok(Self { models, require_complete, raw, aggregated })
            }
            _ => Err(issues),
        }
    }
}

impl Partition {
    fn read(spec: &Map<String, Value>, key: &str, issues: &mut Vec<String>) -> Option<Self> {
        let path = format!("{SPEC}.{key}");
        let Some(Value::Object(map)) = spec.get(key) else {
            issues.push(format!("{path} must be an object"));
            return None;
        };
        let root = kept(text(map, "root", &path), issues);
        let benchmarks = kept(text_list(map, "benchmarks", &path), issues);
        let formats = kept(text_list(map, "formats", &path), issues);
        Some(Self { root: root?, benchmarks: benchmarks?, formats: formats? })
    }
}

/// The value a field read gave, or `None` with its issue recorded.
fn kept<T>(read: Result<T, String>, issues: &mut Vec<String>) -> Option<T> {
    match read {
        Ok(value) => Some(value),
        Err(issue) => {
            issues.push(issue);
            None
        }
    }
}

/// `path.key` as a list of text, refused naming the field when it is
/// absent, not a list, or holds a value that is not text.
fn text_list(map: &Map<String, Value>, key: &str, path: &str) -> Result<Vec<String>, String> {
    match map.get(key) {
        Some(Value::Array(items)) => items
            .iter()
            .map(|item| match item {
                Value::String(text) => Ok(text.clone()),
                other => Err(format!("{path}.{key} holds {other}, which is not text")),
            })
            .collect(),
        Some(other) => Err(format!("{path}.{key} must be a list of text, not {other}")),
        None => Err(format!("{path}.{key} is required: a list of text")),
    }
}

/// `path.key` as non-empty text, refused naming the field otherwise.
fn text(map: &Map<String, Value>, key: &str, path: &str) -> Result<String, String> {
    match map.get(key) {
        Some(Value::String(text)) if !text.is_empty() => Ok(text.clone()),
        Some(other) => Err(format!("{path}.{key} must be non-empty text, not {other}")),
        None => Err(format!("{path}.{key} is required: non-empty text")),
    }
}

/// `path.key` as a JSON boolean, refused naming the field otherwise.
fn flag(map: &Map<String, Value>, key: &str, path: &str) -> Result<bool, String> {
    match map.get(key) {
        Some(Value::Bool(flag)) => Ok(*flag),
        Some(other) => Err(format!("{path}.{key} must be true or false, not {other}")),
        None => Err(format!("{path}.{key} is required: true or false")),
    }
}
