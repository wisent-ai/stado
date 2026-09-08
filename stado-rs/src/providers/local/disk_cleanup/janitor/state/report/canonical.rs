//! Canonical JSON, byte-compatible with the Python original.

use serde_json::Value;

// ---------------------------------------------------------------------------
// canonical JSON (Python json.dumps(sort_keys=True, separators=(",", ":")))
// ---------------------------------------------------------------------------

/// Serialize with recursively sorted object keys and compact separators —
/// byte-compatible with Python's `json.dumps(value, sort_keys=True,
/// separators=(",", ":"))` for the values the janitor emits (ASCII-safe;
/// non-ASCII strings are escaped like Python's default ensure_ascii=True).
pub fn canonical_json(value: &Value) -> String {
    let mut out = String::new();
    write_canonical(value, &mut out);
    out
}

fn write_canonical(value: &Value, out: &mut String) {
    match value {
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            out.push('{');
            for (index, key) in keys.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                write_canonical(&Value::String((*key).clone()), out);
                out.push(':');
                write_canonical(&map[*key], out);
            }
            out.push('}');
        }
        Value::Array(items) => {
            out.push('[');
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                write_canonical(item, out);
            }
            out.push(']');
        }
        // serde_json's own serializer matches json.dumps for scalars;
        // ensure_ascii escaping keeps Python parity for strings.
        other => out.push_str(&crate::models::ensure_ascii(
            &serde_json::to_string(other).unwrap_or_else(|_| "null".to_string()),
        )),
    }
}
