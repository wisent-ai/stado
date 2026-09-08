//! Blob keys for the two families: one path segment per host name, one
//! compact UTC stamp per record.

use chrono::{DateTime, Utc};

use super::{REFUSAL_PREFIX, SILENCE_PREFIX};

/// One path segment from a host name.
///
/// Registry names are already `[a-z0-9-]`, so for every real host this is
/// the identity. It exists for the one that is not: a name carrying `/`
/// would address a different directory, and the local backend would reject
/// the write as a path escape at the moment an operator most needs the
/// record. Everything outside `[A-Za-z0-9._-]` collapses to `-`; the
/// document keeps the host name verbatim regardless.
fn path_segment(host: &str) -> String {
    let mapped: String = host
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                c
            } else {
                '-'
            }
        })
        .collect();
    if mapped.is_empty() {
        "unknown".to_string()
    } else {
        mapped
    }
}

/// Compact UTC stamp used as a blob key.
///
/// Compact rather than RFC-3339 because the key is also a file name on the
/// local-file backend (same rule as `cli/service.rs`'s ensure audit), and
/// microsecond precision because two readers can open the same record in
/// the same second. Lexicographic order over these keys IS chronological
/// order, which is what lets the listing sort without downloading.
fn stamp(at: DateTime<Utc>) -> String {
    at.format("%Y%m%dT%H%M%S%.6fZ").to_string()
}

/// Directory holding every silence record for one host.
pub fn silence_prefix(host: &str) -> String {
    format!("{SILENCE_PREFIX}/{}/", path_segment(host))
}

/// Blob path of one silence record.
pub fn silence_object_path(host: &str, started_at: DateTime<Utc>) -> String {
    format!("{}{}.json", silence_prefix(host), stamp(started_at))
}

/// Directory holding every refusal record about one host.
pub fn refusal_prefix(host: &str) -> String {
    format!("{REFUSAL_PREFIX}/{}/", path_segment(host))
}

/// Blob path of one refusal record.
pub fn refusal_object_path(host: &str, at: DateTime<Utc>) -> String {
    format!("{}{}.json", refusal_prefix(host), stamp(at))
}
