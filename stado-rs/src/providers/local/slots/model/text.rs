//! Python-parity text, JSON and process-status conversions: the env floats
//! admission reads, canonical key-sorted JSON, `Popen.returncode`, and the
//! head/tail slices every record in this module is bounded by.

use super::*;

// ---------------------------------------------------------------------------
// small env / json / process helpers
// ---------------------------------------------------------------------------

/// Python `float(os.environ.get(key, default) or default)`: unset or empty
/// -> default; otherwise float() (whitespace-tolerant). A non-numeric value
/// panics — Python's ValueError crashes the agent at exactly this spot.
pub(crate) fn env_f64(key: &str, default: f64) -> f64 {
    match std::env::var(key) {
        Ok(raw) if !raw.is_empty() => raw
            .trim()
            .parse()
            .unwrap_or_else(|_| panic!("{key} must be a float (Python float() parity): {raw}")),
        _ => default,
    }
}

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
/// was killed by a signal.
pub fn python_returncode(status: std::process::ExitStatus) -> i32 {
    use std::os::unix::process::ExitStatusExt;
    match status.signal() {
        Some(sig) => -sig,
        None => status.code().unwrap_or(0),
    }
}

/// Python `(res.stderr or res.stdout or "")[:n]` on captured bytes.
pub(crate) fn captured_head(stderr: &[u8], stdout: &[u8], n: usize) -> String {
    let text = if !stderr.is_empty() {
        String::from_utf8_lossy(stderr).into_owned()
    } else if !stdout.is_empty() {
        String::from_utf8_lossy(stdout).into_owned()
    } else {
        String::new()
    };
    text.chars().take(n).collect()
}

/// Last `n` chars (Python `s[-n:]`).
pub(crate) fn tail_chars(s: &str, n: usize) -> String {
    s.chars()
        .rev()
        .take(n)
        .collect::<String>()
        .chars()
        .rev()
        .collect()
}

/// First `n` chars (Python `s[:n]`).
pub(crate) fn head_chars(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}
