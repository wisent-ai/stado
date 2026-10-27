//! Blob keys for the two families: one path segment per host name, one
//! compact UTC stamp per record.

use chrono::{DateTime, Utc};

use super::{REFUSAL_PREFIX, SILENCE_PREFIX};
use crate::queue::StorageError;

/// One path segment from a host name.
///
/// Registry names are already `[a-z0-9-]`, so for every real host this is
/// the identity. It exists for the one that is not: a name carrying `/`
/// would address a different directory, and the local backend would reject
/// the write as a path escape at the moment an operator most needs the
/// record. Everything outside `[A-Za-z0-9._-]` collapses to `-`; the
/// document keeps the host name verbatim regardless. An empty name names no
/// host, so no record can be kept or read for it: refused by name.
fn path_segment(host: &str) -> Result<String, StorageError> {
    if host.is_empty() {
        return Err(StorageError::Other(
            "a host silence or refusal record names its host, and this one's host name is empty".to_string(),
        ));
    }
    Ok(host
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') { c } else { '-' })
        .collect())
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
pub fn silence_prefix(host: &str) -> Result<String, StorageError> {
    Ok(format!("{SILENCE_PREFIX}/{}/", path_segment(host)?))
}

/// Blob path of one silence record.
pub fn silence_object_path(host: &str, started_at: DateTime<Utc>) -> Result<String, StorageError> {
    Ok(format!("{}{}.json", silence_prefix(host)?, stamp(started_at)))
}

/// Directory holding every refusal record about one host.
pub fn refusal_prefix(host: &str) -> Result<String, StorageError> {
    Ok(format!("{REFUSAL_PREFIX}/{}/", path_segment(host)?))
}

/// Blob path of one refusal record.
pub fn refusal_object_path(host: &str, at: DateTime<Utc>) -> Result<String, StorageError> {
    Ok(format!("{}{}.json", refusal_prefix(host)?, stamp(at)))
}
