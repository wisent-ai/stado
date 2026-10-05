use std::collections::BTreeMap;

use futures::StreamExt;
use serde_json::{Map, Value};

use super::orchestrator::state_slot;
use super::{state_load, state_save, CoverageError, Universe};
use crate::queue::JobStorage;

/// Python `FAILED_PREFIX`.
pub const FAILED_PREFIX: &str = "failed/";
/// Python `_load_failed_blob`: corrupt/absent blobs become None.
async fn load_failed_blob(store: &JobStorage, path: &str) -> Result<Option<Value>, CoverageError> {
    let Some(txt) = store.download_text(path).await? else {
        return Ok(None);
    };
    Ok(serde_json::from_str(&txt).ok())
}

/// Python `scan_failed_commands`: `{command: most_recent_failure_record}`
/// from `failed/`. With `command_prefix`, only failed blobs whose
/// `.command` starts with the prefix are kept. The
/// most-recent-by-failed_at record wins on duplicate commands.
pub async fn scan_failed_commands(
    store: &JobStorage,
    command_prefix: Option<&str>,
    threads: usize,
) -> Result<BTreeMap<String, Map<String, Value>>, CoverageError> {
    let infos = store.list_blobs_with_meta(FAILED_PREFIX).await?;
    let paths: Vec<String> = infos
        .into_iter()
        .map(|info| info.name)
        .filter(|name| name.ends_with(".json"))
        .collect();
    let blobs: Vec<Result<Option<Value>, CoverageError>> = futures::stream::iter(paths)
        .map(|path| async move { load_failed_blob(store, &path).await })
        .buffer_unordered(threads)
        .collect()
        .await;
    let mut out: BTreeMap<String, Map<String, Value>> = BTreeMap::new();
    for blob in blobs {
        let Some(blob) = blob? else { continue };
        let Some(cmd) = blob
            .get("command")
            .and_then(Value::as_str)
            .filter(|cmd| !cmd.is_empty())
        else {
            continue;
        };
        if let Some(prefix) = command_prefix {
            if !cmd.starts_with(prefix) {
                continue;
            }
        }
        let ts = blob
            .get("failed_at")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let wins = match out.get(cmd) {
            None => true,
            Some(prev) => prev.get("failed_at").and_then(Value::as_str).unwrap_or("") < ts.as_str(),
        };
        if !wins {
            continue;
        }
        let error = blob.get("error").and_then(Value::as_str).unwrap_or("");
        let record = Map::from_iter([
            ("error".to_string(), Value::from(error.to_string())),
            ("failed_at".to_string(), Value::from(ts)),
            (
                "job_id".to_string(),
                Value::from(blob.get("job_id").and_then(Value::as_str).unwrap_or("")),
            ),
            (
                "batch_id".to_string(),
                Value::from(blob.get("batch_id").and_then(Value::as_str).unwrap_or("")),
            ),
        ]);
        out.insert(cmd.to_string(), record);
    }
    Ok(out)
}

/// Python `correlate_failures_into_state`: pre-seed the universe's
/// coverage state with last_error/last_failure_at pulled from the
/// failed/ index. A failure from a different job than the one recorded
/// before, carrying the same error, sets `repeated_error`: two submissions
/// failed the same way, so a third would too, and the next `verify`
/// promotes the group_key to UNFIXABLE with that error. Returns the merged
/// state (also persisted to storage when anything matched). Unlike the
/// Python signature the store is required (the `None` default only
/// constructed `JobStorage(BUCKET)`).
pub async fn correlate_failures_into_state(
    universe: &dyn Universe,
    store: &JobStorage,
    state: Option<Value>,
    command_prefix: Option<&str>,
) -> Result<Value, CoverageError> {
    let mut state = match state {
        Some(state) => state,
        None => state_load(store, universe.id()).await?,
    };
    let failed = scan_failed_commands(
        store,
        command_prefix,
        crate::queue::migrations::bulk_workers(),
    )
    .await?;
    if failed.is_empty() {
        return Ok(state);
    }
    let mut matched = 0usize;
    for entry in universe.iter_entries() {
        let Some(rec) = failed.get(&entry.command) else {
            continue;
        };
        let slot = state_slot(&mut state, &entry.group_key);
        if slot.get("last_failed_job_id") != Some(&rec["job_id"]) {
            let repeated = slot.get("last_error") == Some(&rec["error"]);
            slot.insert("repeated_error".into(), Value::from(repeated));
        }
        slot.insert("last_error".into(), rec["error"].clone());
        slot.insert("last_failure_at".into(), rec["failed_at"].clone());
        slot.insert("last_failed_job_id".into(), rec["job_id"].clone());
        slot.insert("last_failed_batch_id".into(), rec["batch_id"].clone());
        matched += 1;
    }
    if matched > 0 {
        state_save(store, universe.id(), &state).await?;
    }
    Ok(state)
}

/// Python `matched_failed_jids_for_universe`: `{group_key:
/// failed_job_id}` for entries whose command has a matching failed/
/// blob.
pub async fn matched_failed_jids_for_universe(
    universe: &dyn Universe,
    store: &JobStorage,
    command_prefix: Option<&str>,
) -> Result<BTreeMap<String, String>, CoverageError> {
    let failed = scan_failed_commands(
        store,
        command_prefix,
        crate::queue::migrations::bulk_workers(),
    )
    .await?;
    let mut out = BTreeMap::new();
    for entry in universe.iter_entries() {
        if let Some(rec) = failed.get(&entry.command) {
            out.insert(
                entry.group_key.clone(),
                rec.get("job_id")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
            );
        }
    }
    Ok(out)
}

/// Python `iter_failed_commands`: the (command, failure_record) pairs
/// of [`scan_failed_commands`], as a streaming-iterator analog.
pub async fn iter_failed_commands(
    store: &JobStorage,
    command_prefix: Option<&str>,
) -> Result<Vec<(String, Map<String, Value>)>, CoverageError> {
    let scanned = scan_failed_commands(
        store,
        command_prefix,
        crate::queue::migrations::bulk_workers(),
    )
    .await?;
    Ok(scanned.into_iter().collect())
}
