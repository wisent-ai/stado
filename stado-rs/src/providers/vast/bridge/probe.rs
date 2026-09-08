//! Queue-state probes: the busy/idle decision inputs the auto-list loop
//! polls, and the capacity blob read the CLI reports as a diagnostic.
//!
//! Moved verbatim out of the former single-file `providers/vast`.

use serde_json::{json, Value};

use crate::queue::{JobStorage, StorageError};

/// Python `_is_stado_busy` result.
#[derive(Debug, Clone, PartialEq)]
pub struct BusyState {
    pub queued: usize,
    pub running_here: usize,
    pub free_vram_gb: Option<f64>,
    pub idle: bool,
}

/// Python `_is_stado_busy`: busy = queue not empty OR any running/ blob has
/// instance_ref referencing this hostname. (The earlier impl read
/// claimed_this_loop — a per-iter counter, ~always 0 — and missed in-flight
/// work.) Corrupt or vanished blobs are skipped, like Python's blanket
/// `except (NotFound, Exception): continue`.
pub async fn is_stado_busy(store: &JobStorage, hostname: &str) -> Result<BusyState, StorageError> {
    // Python list_blobs(prefix="queue/", max_results=2) — capped at 2.
    let queued = store.list_paths("queue/", 2).await?.len();
    let mut running_here = 0;
    for path in store.list_paths("running/", 0).await? {
        let Ok(Some(text)) = store.download_text(&path).await else {
            continue;
        };
        let Ok(doc) = serde_json::from_str::<Value>(&text) else {
            continue;
        };
        let instance_ref = doc
            .get("instance_ref")
            .and_then(Value::as_str)
            .unwrap_or("");
        if !hostname.is_empty() && instance_ref.contains(hostname) {
            running_here += 1;
        }
    }
    let free_vram_gb = match store
        .download_text(&format!("capacity/local-{hostname}.json"))
        .await
    {
        Ok(Some(text)) => serde_json::from_str::<Value>(&text)
            .ok()
            .and_then(|doc| doc.get("free_vram_gb").and_then(Value::as_f64)),
        _ => None,
    };
    Ok(BusyState {
        queued,
        running_here,
        free_vram_gb,
        idle: queued == 0 && running_here == 0,
    })
}

/// The monitor snapshot capacity-read helper shared with the CLI: parse
/// `capacity/local-{hostname}.json`, mirroring the Python error records.
pub async fn read_capacity_snapshot(store: &JobStorage, hostname: &str) -> Value {
    let path = format!("capacity/local-{hostname}.json");
    match store.download_text(&path).await {
        Ok(Some(text)) => serde_json::from_str(&text)
            .unwrap_or_else(|exc| json!({"error": format!("JSONDecodeError: {exc}")})),
        Ok(None) => json!({"error": format!("{path} not found")}),
        Err(exc) => json!({"error": format!("{exc}")}),
    }
}
