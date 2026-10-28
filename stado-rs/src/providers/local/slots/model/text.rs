//! Python-parity text, JSON and process-status conversions: canonical
//! key-sorted JSON, `Popen.returncode`, and character-based head slices.

use super::*;

// ---------------------------------------------------------------------------
// small env / json / process helpers
// ---------------------------------------------------------------------------

/// Recursively key-sorted compact JSON (Python
/// `json.dumps(d, sort_keys=True, separators=(",", ":"))`, ensure_ascii=True).
pub fn canonical_json(value: &Value) -> String {
    crate::models::ensure_ascii(
        &serde_json::to_string(&sort_keys(value)).expect("Value serialization is infallible"),
    )
}

fn sort_keys(value: &Value) -> Value {
    match value {
        Value::Object(map) => {
            let mut entries: Vec<(&String, &Value)> = map.iter().collect();
            entries.sort_by(|a, b| a.0.cmp(b.0));
            Value::Object(
                entries
                    .into_iter()
                    .map(|(k, v)| (k.clone(), sort_keys(v)))
                    .collect(),
            )
        }
        Value::Array(items) => Value::Array(items.iter().map(sort_keys).collect()),
        other => other.clone(),
    }
}

/// Python `Popen.returncode`: the exit code, or `-signum` when the child
/// was killed by a signal. A status a wait hands back is one or the other;
/// one that is neither is not read as a success.
pub fn python_returncode(status: std::process::ExitStatus) -> i32 {
    use std::os::unix::process::ExitStatusExt;
    match (status.code(), status.signal()) {
        (Some(code), _) => code,
        (None, Some(sig)) => -sig,
        (None, None) => unreachable!(
            "a finished child either exited or was ended by a signal, and {status} is neither"
        ),
    }
}
