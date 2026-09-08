//! Location/shard patterns and the Python-value helpers the activation
//! adapter reads its specification with.

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

/// Python `bool(value)` on a JSON value (for `require_complete_markers`).
pub(super) fn py_truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64() != Some(0.0),
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(m) => !m.is_empty(),
    }
}

/// Python `value.get(key, [])` iterated as a list of strings. Non-array
/// values degrade to empty (Python would iterate dict keys / string chars
/// — pathological input the Rust port refuses to emulate; noted deviation).
pub(super) fn str_list(map: &Map<String, Value>, key: &str) -> Vec<String> {
    map.get(key)
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|v| v.as_str().map(ToString::to_string))
                .collect()
        })
        .unwrap_or_default()
}
