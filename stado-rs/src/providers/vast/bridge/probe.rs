//! Queue-state probes: the busy/idle decision inputs the auto-list loop
//! polls, and the capacity blob read the CLI reports as a diagnostic.
//!
//! Moved out of the former single-file `providers/vast`.

use std::num::NonZeroUsize;

use serde_json::{json, Value};

use crate::queue::{JobStorage, Listing, StorageError};

/// Python `_is_stado_busy` result.
#[derive(Debug, Clone, PartialEq)]
pub struct BusyState {
    /// Queued jobs the probe saw. It reads one name of the queue, which is
    /// all the busy decision needs, so this is none or one, never a count of
    /// the whole queue.
    pub queued: usize,
    /// Running jobs whose instance names this host.
    pub running_here: usize,
    pub free_vram_gb: Option<f64>,
    pub idle: bool,
}

/// Python `_is_stado_busy`: busy = queue not empty OR any running/ blob has
/// instance_ref referencing this hostname. A blob that vanished between the
/// listing and the read is skipped; one that cannot be read or does not parse
/// is the error, because skipped it would count as work running nowhere and
/// could call a busy host idle, which hands it to the marketplace. A running job
/// that names no instance runs on no host yet, so not on this one.
pub async fn is_stado_busy(store: &JobStorage, hostname: &str) -> Result<BusyState, StorageError> {
    let queued = store
        .list_page("queue/", "", Some(NonZeroUsize::MIN))
        .await?;
    let mut running_here: Vec<String> = Vec::new();
    for path in store.list_paths("running/", Listing::Every).await? {
        let Some(text) = store.download_text(&path).await? else {
            continue;
        };
        let doc = serde_json::from_str::<Value>(&text).map_err(|error| {
            StorageError::Other(format!(
                "{path} is not a job record, so whether it runs on {hostname} cannot be read: {error}"
            ))
        })?;
        let here = match doc.get("instance_ref").and_then(Value::as_str) {
            Some(instance_ref) => !hostname.is_empty() && instance_ref.contains(hostname),
            None => false,
        };
        if here {
            running_here.push(path);
        }
    }
    // A host that publishes no capacity has no free memory to report; one
    // whose capacity cannot be read or parsed is the error.
    let capacity = format!("capacity/local-{hostname}.json");
    let free_vram_gb = match store.download_text(&capacity).await? {
        None => None,
        Some(text) => {
            let doc = serde_json::from_str::<Value>(&text).map_err(|error| {
                StorageError::Other(format!("{capacity} is not a capacity record: {error}"))
            })?;
            doc.get("free_vram_gb").and_then(Value::as_f64)
        }
    };
    Ok(BusyState {
        queued: queued.len(),
        running_here: running_here.len(),
        free_vram_gb,
        idle: queued.is_empty() && running_here.is_empty(),
    })
}

/// The monitor snapshot capacity-read helper shared with the CLI: parse
/// `capacity/local-{hostname}.json`, mirroring the Python error records.
pub async fn read_capacity_snapshot(store: &JobStorage, hostname: &str) -> Value {
    let path = format!("capacity/local-{hostname}.json");
    match store.download_text(&path).await {
        Ok(Some(text)) => match serde_json::from_str(&text) {
            Ok(snapshot) => snapshot,
            Err(exc) => json!({"error": format!("JSONDecodeError: {exc}")}),
        },
        Ok(None) => json!({"error": format!("{path} not found")}),
        Err(exc) => json!({"error": format!("{exc}")}),
    }
}
