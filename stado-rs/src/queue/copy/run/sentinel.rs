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

/// Read the destination sentinel; a missing, empty or unparseable body
/// starts from scratch (same tolerance as `migrations::read_sentinel`).
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
    let number = |key: &str| {
        value
            .get(key)
            .and_then(serde_json::Value::as_u64)
            .unwrap_or_default()
    };
    Ok(Sentinel {
        cursor: value
            .get("cursor")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_string(),
        copied: number("copied"),
        repaired: number("repaired"),
        skipped: number("skipped"),
        vanished: number("vanished"),
        failed: number("failed"),
        bytes: number("bytes"),
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
