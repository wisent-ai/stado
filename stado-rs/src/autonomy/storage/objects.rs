//! One autonomy object read or written by type, and the prefix listings every
//! record component shares.

use std::collections::BTreeSet;

use chrono::{DateTime, Utc};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::queue::{JobStorage, StorageError};

pub async fn write_json<T: Serialize>(
    store: &JobStorage,
    path: &str,
    value: &T,
    immutable: bool,
) -> Result<(), StorageError> {
    let content = serde_json::to_string(value)?;
    if !immutable {
        return store.upload_text(path, &content).await;
    }
    if store.create_text_if_absent(path, &content).await? {
        Ok(())
    } else {
        Err(StorageError::StorageConflict(format!(
            "immutable autonomy object already exists: {path}"
        )))
    }
}

pub async fn read_json<T: DeserializeOwned>(
    store: &JobStorage,
    path: &str,
) -> Result<Option<T>, StorageError> {
    let Some(content) = store.download_text(path).await? else {
        return Ok(None);
    };
    Ok(Some(serde_json::from_str(&content)?))
}

/// Which record ids exist under one prefix, and when each was last written —
/// from a single listing, with no body downloaded.
///
/// Every record type in this module is keyed by its own id in its object name,
/// so "does this id exist" and "how old is it" are questions the listing
/// already answers. Asking them by downloading each body is what made one
/// coordinator tick 11,514 serial object GETs on 2026-09-02 — 8,129 decisions
/// and 3,385 feedback records, re-read every tick for work finished months
/// earlier. The fleet store serves the release channel too, so that tick is
/// what starved the 0.13.42 release download to 570 KB/s.
pub(in crate::autonomy::storage) async fn list_record_index(
    store: &JobStorage,
    prefix: &str,
) -> Result<Vec<(String, Option<DateTime<Utc>>)>, StorageError> {
    let blobs = store.list_blobs_with_meta(prefix).await?;
    let mut index = Vec::with_capacity(blobs.len());
    for blob in blobs {
        let name = blob.name.rsplit('/').next().unwrap_or_default();
        let Some(id) = name.strip_suffix(".json") else {
            continue;
        };
        if id.is_empty() {
            continue;
        }
        index.push((id.to_string(), blob.updated));
    }
    Ok(index)
}

pub(in crate::autonomy::storage) async fn list_record_ids(
    store: &JobStorage,
    prefix: &str,
) -> Result<BTreeSet<String>, StorageError> {
    Ok(list_record_index(store, prefix)
        .await?
        .into_iter()
        .map(|(id, _)| id)
        .collect())
}

pub(in crate::autonomy::storage) async fn load_records<T>(
    store: &JobStorage,
    prefix: &str,
) -> Result<Vec<T>, StorageError>
where
    T: for<'de> Deserialize<'de>,
{
    let mut records = Vec::new();
    for path in store.list_paths(prefix, usize::default()).await? {
        let Some(raw) = store.download_text(&path).await? else {
            continue;
        };
        records.push(serde_json::from_str(&raw)?);
    }
    Ok(records)
}
