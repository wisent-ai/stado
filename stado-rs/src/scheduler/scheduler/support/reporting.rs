//! Stderr reporting in the Python Cloud Function's exact output format, so
//! a Rust tick's log is diffable against a Python tick's.

use std::collections::BTreeMap;

/// Python `_log`.
pub(crate) fn log(msg: &str) {
    eprintln!("[scheduler] {msg}");
}

/// Python dict repr for `BTreeMap<String, i64>` (`{'a':
/// 1, 'b':
/// 2}`), so
/// stderr logs read exactly like the Python Cloud Function's.
pub(crate) fn py_dict_i64(map: &BTreeMap<String, i64>) -> String {
    let inner: Vec<String> = map.iter().map(|(k, v)| format!("'{k}': {v}")).collect();
    format!("{{{}}}", inner.join(", "))
}

/// Python dict repr for insertion-ordered `(String, i64)` pairs
/// (consumers_by_free_vram order).
pub(crate) fn py_pairs_i64(pairs: &[(String, i64)]) -> String {
    let inner: Vec<String> = pairs.iter().map(|(k, v)| format!("'{k}': {v}")).collect();
    format!("{{{}}}", inner.join(", "))
}
