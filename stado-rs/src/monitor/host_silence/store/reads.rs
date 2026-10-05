//! Reading the two families back: the newest silences for a host, the one
//! that is still open, and the refusals inside a window.

use chrono::{DateTime, Utc};

use crate::monitor::host_silence::paths::{refusal_prefix, silence_prefix};
use crate::monitor::host_silence::records::{RefusalRecord, RefusalSummary, SilenceRecord};
use crate::monitor::host_silence::transitions::summarize_refusals;
use crate::queue::{JobStorage, StorageError};

/// Blob paths under `prefix`, newest key first.
///
/// The key is a compact UTC stamp, so a reverse lexicographic sort is a
/// reverse chronological sort and the caller downloads only the documents
/// it is going to keep.
async fn newest_first(store: &JobStorage, prefix: &str) -> Result<Vec<String>, StorageError> {
    let mut paths = store.list_paths(prefix, 0).await?;
    paths.retain(|path| path.ends_with(".json"));
    paths.sort_unstable_by(|left, right| right.cmp(left));
    Ok(paths)
}

/// Parse one stored document, or `None` when it is absent or unreadable.
///
/// A corrupt record is skipped rather than propagated. These blobs are read
/// while something is already broken; refusing to report five silences
/// because one of them was truncated by a host that lost power mid-write is
/// the diagnostic failing for the same reason as its subject.
pub(super) async fn read_document<T: serde::de::DeserializeOwned>(
    store: &JobStorage,
    path: &str,
) -> Result<Option<T>, StorageError> {
    let Some(body) = store.download_text(path).await? else {
        return Ok(None);
    };
    Ok(serde_json::from_str(&body).ok())
}

/// Every silence record for `host`, newest first.
pub async fn silences(store: &JobStorage, host: &str) -> Result<Vec<SilenceRecord>, StorageError> {
    let mut out = Vec::new();
    for path in newest_first(store, &silence_prefix(host)).await? {
        if let Some(record) = read_document::<SilenceRecord>(store, &path).await? {
            out.push(record);
        }
    }
    Ok(out)
}

/// The currently open silence for `host`, if the host is inside one.
///
/// Only the newest record can be open: a silence opens only when none is
/// open, and closing writes back to the same key. An older record still
/// carrying a null `ended_at` is a crashed writer, not a second live gap,
/// and is deliberately left alone rather than retro-closed with a time
/// nobody observed.
pub async fn open_silence(
    store: &JobStorage,
    host: &str,
) -> Result<Option<(String, SilenceRecord)>, StorageError> {
    let Some(path) = newest_first(store, &silence_prefix(host))
        .await?
        .into_iter()
        .next()
    else {
        return Ok(None);
    };
    let Some(record) = read_document::<SilenceRecord>(store, &path).await? else {
        return Ok(None);
    };
    if record.ended_at.is_some() {
        return Ok(None);
    }
    Ok(Some((path, record)))
}

/// Every refusal about `host` at or after `since`, newest first; every
/// retained refusal when `since` is `None`.
pub async fn refusals_since(
    store: &JobStorage,
    host: &str,
    since: Option<DateTime<Utc>>,
) -> Result<Vec<RefusalRecord>, StorageError> {
    let mut out = Vec::new();
    for path in newest_first(store, &refusal_prefix(host)).await? {
        let Some(record) = read_document::<RefusalRecord>(store, &path).await? else {
            continue;
        };
        // Keys sort chronologically, so the first record before `since` ends
        // the walk: nothing behind it can be newer.
        if since.is_some_and(|since| record.at < since) {
            break;
        }
        out.push(record);
    }
    Ok(out)
}

/// Refusal count and per-reason counts about `host` at or after `since`.
pub async fn refusal_summary(
    store: &JobStorage,
    host: &str,
    since: Option<DateTime<Utc>>,
) -> Result<RefusalSummary, StorageError> {
    let records = refusals_since(store, host, since).await?;
    Ok(summarize_refusals(&records, since))
}
