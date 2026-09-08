//! Canonical-JSON helpers behind [`super::ArtifactManifest::to_json`].

use std::collections::BTreeMap;

use serde_json::{Map, Value};

/// Rebuild a JSON value with every object's keys in sorted order (the
/// crate enables serde_json's `preserve_order`, so insertion order is
/// serialization order).
pub(super) fn canonicalize(value: &Value) -> Value {
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
pub(super) fn ensure_ascii(s: &str) -> String {
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
