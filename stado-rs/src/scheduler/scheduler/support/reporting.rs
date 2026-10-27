//! Stderr reporting in the Python Cloud Function's exact output format, so
//! a Rust tick's log is diffable against a Python tick's.

use std::collections::BTreeMap;

/// Python `_log`.
pub(crate) fn log(msg: &str) {
    eprintln!("[scheduler] {msg}");
}

/// Python dict repr of string keys and whole numbers, in the order given.
/// Each key is quoted by the one `py_str_repr`, so a key holding a quote
/// reads back as itself.
fn py_int_dict<'a>(pairs: impl Iterator<Item = (&'a String, &'a i64)>) -> String {
    let inner: Vec<String> = pairs
        .map(|(key, value)| format!("{}: {value}", crate::models::py_str_repr(key)))
        .collect();
    format!("{{{}}}", inner.join(", "))
}

/// Python dict repr for `BTreeMap<String, i64>`, so stderr logs read exactly
/// like the Python Cloud Function's.
pub(crate) fn py_dict_i64(map: &BTreeMap<String, i64>) -> String {
    py_int_dict(map.iter())
}

/// Python dict repr for insertion-ordered `(String, i64)` pairs
/// (consumers_by_claimable_vram order).
pub(crate) fn py_pairs_i64(pairs: &[(String, i64)]) -> String {
    py_int_dict(pairs.iter().map(|(key, value)| (key, value)))
}
