//! The exact Python encodings the envelope and the request digest depend on.

use std::collections::BTreeMap;

use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

/// `datetime.now(timezone.utc).isoformat()`.
pub(crate) fn utcnow() -> String {
    crate::models::isoformat_utc(chrono::Utc::now())
}

/// Python `repr()` of a simple string: single quotes, switching to double
/// quotes when the string contains a single quote (job ids are hex, so the
/// escape corner cases of repr never trigger).
pub(in crate::machine) fn py_repr(s: &str) -> String {
    if s.contains('\'') {
        format!("\"{s}\"")
    } else {
        format!("'{s}'")
    }
}

/// Python `json.dumps(value, ensure_ascii=False, sort_keys=True,
/// separators=(",", ":"))`: compact separators, keys sorted recursively,
/// non-ASCII left as raw UTF-8 (unlike `submit::json_dumps_sorted_compact`,
/// which matches the ensure_ascii=True default used elsewhere).
pub fn canonical_json(value: &Value) -> String {
    fn sorted(value: &Value) -> Value {
        match value {
            Value::Object(map) => {
                let btree: BTreeMap<String, Value> = map
                    .iter()
                    .map(|(key, value)| (key.clone(), sorted(value)))
                    .collect();
                Value::Object(btree.into_iter().collect())
            }
            Value::Array(items) => Value::Array(items.iter().map(sorted).collect()),
            other => other.clone(),
        }
    }
    serde_json::to_string(&sorted(value)).expect("JSON serialization is infallible")
}

/// SHA-256 hex of the canonical request JSON (Python `_request_digest`).
pub(in crate::machine) fn request_digest(request: &Map<String, Value>) -> String {
    hex::encode(Sha256::digest(
        canonical_json(&Value::Object(request.clone())).as_bytes(),
    ))
}
