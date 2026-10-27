//! The destination resume marker: its body shape and its two round trips.

use std::sync::Arc;

use crate::queue::copy::SENTINEL_PATH;
use crate::queue::{python_json_dumps, BlobBackend, StorageError};

/// Sentinel body: resume cursor plus cumulative counts across runs.
#[derive(Clone, Debug, Default)]
pub(super) struct Sentinel {
    pub(super) cursor: String,
    pub(super) copied: u64,
    pub(super) repaired: u64,
    pub(super) skipped: u64,
    pub(super) vanished: u64,
    pub(super) failed: u64,
    pub(super) bytes: u64,
}

/// Read the destination sentinel; a missing or empty body starts from
/// scratch, a body that is not JSON or holds a member of the wrong type is
/// refused naming the member, since resuming from it would miscount or
/// restart a copy that was far along.
pub(super) async fn read_sentinel(
    destination: &Arc<dyn BlobBackend>,
) -> Result<Sentinel, StorageError> {
    let Some(raw) = destination.download_text(SENTINEL_PATH).await? else {
        return Ok(Sentinel::default());
    };
    if raw.is_empty() {
        return Ok(Sentinel::default());
    }
    let value: serde_json::Value = serde_json::from_str(&raw)?;
    // The sentinel is what a previous copy wrote: a member it did not write
    // (an older sentinel) stays as a fresh sentinel has it, but a member of
    // another type is a sentinel nobody wrote and is refused, never read as
    // a cursor at the start or a count of nothing.
    let fresh = Sentinel::default();
    let number = |key: &str, fresh: u64| -> Result<u64, StorageError> {
        match value.get(key) {
            None | Some(serde_json::Value::Null) => Ok(fresh),
            Some(member) => member.as_u64().ok_or_else(|| {
                StorageError::Other(format!(
                    "the copy sentinel at {SENTINEL_PATH} holds {key} = {member}, not a count"
                ))
            }),
        }
    };
    let cursor = match value.get("cursor") {
        None | Some(serde_json::Value::Null) => fresh.cursor.clone(),
        Some(serde_json::Value::String(cursor)) => cursor.clone(),
        Some(other) => {
            return Err(StorageError::Other(format!(
                "the copy sentinel at {SENTINEL_PATH} holds cursor = {other}, not an object name"
            )))
        }
    };
    Ok(Sentinel {
        cursor,
        copied: number("copied", fresh.copied)?,
        repaired: number("repaired", fresh.repaired)?,
        skipped: number("skipped", fresh.skipped)?,
        vanished: number("vanished", fresh.vanished)?,
        failed: number("failed", fresh.failed)?,
        bytes: number("bytes", fresh.bytes)?,
    })
}

/// Persist the sentinel to the destination (Python-compatible `json.dumps`
/// separators, like every other JSON body this crate writes).
pub(super) async fn write_sentinel(
    destination: &Arc<dyn BlobBackend>,
    state: &Sentinel,
) -> Result<(), StorageError> {
    let body = python_json_dumps(&serde_json::json!({
        "cursor": state.cursor,
        "copied": state.copied,
        "repaired": state.repaired,
        "skipped": state.skipped,
        "vanished": state.vanished,
        "failed": state.failed,
        "bytes": state.bytes,
    }))?;
    destination.upload_text(SENTINEL_PATH, &body).await
}
