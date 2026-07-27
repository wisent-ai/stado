//! Capacity broadcast channel between wisent-compute consumers.
//!
//! Port of `stado/queue/capacity.py`.
//!
//! Each consumer (cloud function dispatcher, local agent, vast.ai worker, ...)
//! publishes its current free-slots-by-accelerator-type to the bucket on every
//! poll/tick. Other consumers read all live (non-stale) publications to
//! decide whether to yield to a peer that has more capacity.
//!
//! Scheme:
//!   <bucket>/capacity/<consumer_id>.json
//!   {
//!     "consumer_id": "local-rtx-pro-6000-1",
//!     "kind": "local",                       # or "gcp", "aws", ...
//!     "free_slots": {"nvidia-tesla-a100": 1, "nvidia-l4": 0},
//!     "published_at": "2026-04-25T17:42:00.000Z"
//!   }
//!
//! A publication older than CAPACITY_STALE_SECONDS is ignored.
//!
//! Known Python bug (ported as INTENDED, not as written): `capacity.py:121`
//! raises unless `store._sdk_bucket or store._azure_backend` — the latter an
//! attribute that never exists on Python `JobStorage` (the backend handle is
//! `_blob_backend`). The intended precondition is "the backend can list
//! blobs with metadata", which holds for every Rust `BlobBackend`, so no
//! gate exists here.

use std::collections::BTreeMap;

use chrono::{DateTime, Duration, Utc};
use serde_json::{Map, Value};

use crate::constants;

use super::storage::JobStorage;
use super::StorageError;

/// Python `CAPACITY_PREFIX`.
pub const CAPACITY_PREFIX: &str = "capacity/";
/// Python `CAPACITY_STALE_SECONDS = _wc.CAPACITY_STALE_SECONDS`. (The crate
/// also has `constants::LIVE_CAPACITY_TTL_S`, the same 180s, used by other
/// live-capacity readers.)
pub const CAPACITY_STALE_SECONDS: u64 = constants::CAPACITY_STALE_SECONDS;
/// Long-stale cutoff for GC (Python `now.timestamp() - 3600`).
const CAPACITY_GC_AGE_SECONDS: i64 = 3600;
/// GC is capped per tick so the Cloud Function never spends its budget on GC.
const CAPACITY_GC_CAP_PER_TICK: usize = 200;

/// Write this consumer's current capacity snapshot. Python
/// `publish_capacity`.
///
/// `free_vram_gb` is the authoritative admission signal for local consumers:
/// the scheduler yields a queued job whose gpu_mem_gb fits in this number,
/// instead of decrementing a flat per-accel slot counter that ignores the
/// job's actual memory footprint. `free_slots` is kept for backward compat
/// with consumers that haven't been upgraded yet.
///
/// `diag` carries per-tick claim-loop telemetry so a reaper or dashboard can
/// see why a broadcasting agent isn't claiming. Keys:
///   queue_scanned         # of queued jobs the agent inspected this loop
///   vram_rejected         # rejected because need > free_vram_gb
///   eligibility_rejected  # rejected by _job_eligible (incl. LOCAL_ONLY)
///   eligible_count        # passed both gates
///   claimed_this_loop     # actually start_slot()'d this iteration
///   last_started_job_id   # most recent job_id this agent moved to running/
///   last_started_at       # ISO ts of last successful start_slot
///   last_claim_attempt_at # ISO ts of this loop iteration
///
/// Python also broadcasts `stado_version` (importlib.metadata, the version
/// pip actually installed) — here the crate version is the equivalent — and
/// `vast_bridge_active` / `vast_api_key_present` from the providers.vast
/// module, which is not ported yet (TODO(phase-2)); those two keys are
/// omitted rather than reported as false.
pub async fn publish_capacity(
    store: &JobStorage,
    consumer_id: &str,
    kind: &str,
    free_slots: &BTreeMap<String, i64>,
    free_vram_gb: Option<i64>,
    total_vram_gb: Option<i64>,
    diag: Option<Map<String, Value>>,
) -> Result<(), StorageError> {
    let mut payload = Map::new();
    payload.insert("consumer_id".into(), Value::String(consumer_id.to_string()));
    payload.insert("kind".into(), Value::String(kind.to_string()));
    payload.insert(
        "free_slots".into(),
        Value::Object(
            free_slots
                .iter()
                .map(|(accel, n)| (accel.clone(), Value::from(*n)))
                .collect(),
        ),
    );
    payload.insert("published_at".into(), Value::String(Utc::now().to_rfc3339()));
    if let Some(v) = free_vram_gb {
        payload.insert("free_vram_gb".into(), Value::from(v));
    }
    if let Some(v) = total_vram_gb {
        payload.insert("total_vram_gb".into(), Value::from(v));
    }
    if let Some(diag) = diag {
        payload.insert("diag".into(), Value::Object(diag));
    }
    payload.insert("stado_version".into(), Value::String(env!("CARGO_PKG_VERSION").to_string()));
    let body = super::python_json_dumps(&Value::Object(payload))?;
    store.upload_text(&format!("{CAPACITY_PREFIX}{consumer_id}.json"), &body).await
}

/// Return {consumer_id: payload} for every live (non-stale) consumer.
/// Python `read_consumer_capacity`.
pub async fn read_consumer_capacity(
    store: &JobStorage,
) -> Result<BTreeMap<String, Value>, StorageError> {
    read_consumer_capacity_at(store, Utc::now()).await
}

/// [`read_consumer_capacity`] with an injectable clock so the staleness and
/// GC windows are testable without backdating blob mtimes (Python reads the
/// clock inline).
///
/// Filters on blob.updated metadata BEFORE downloading. Previously every
/// tick downloaded all broadcast files (1900+ accumulated, most stale)
/// just to read published_at — at ~30ms/blob this exceeded the 60s Cloud
/// Function timeout, returned 504, and Cloud Scheduler auto-paused the
/// cron. Filtering on server-side metadata first means the tick reads
/// only the small number of fresh blobs.
///
/// Also deletes long-stale blobs (older than 1h, well past
/// CAPACITY_STALE_SECONDS=180s) so the bucket can't accumulate forever.
/// Capped per tick so the Cloud Function never spends its budget on GC.
async fn read_consumer_capacity_at(
    store: &JobStorage,
    now: DateTime<Utc>,
) -> Result<BTreeMap<String, Value>, StorageError> {
    let cutoff_fresh = now - Duration::seconds(CAPACITY_STALE_SECONDS as i64);
    let cutoff_delete = now - Duration::seconds(CAPACITY_GC_AGE_SECONDS);

    let mut out: BTreeMap<String, Value> = BTreeMap::new();
    let mut stale_blobs: Vec<String> = Vec::new();
    for blob in store.list_blobs_with_meta(CAPACITY_PREFIX).await? {
        if !blob.name.ends_with(".json") {
            continue;
        }
        let Some(updated) = blob.updated else { continue };
        if updated < cutoff_delete {
            stale_blobs.push(blob.name);
            continue;
        }
        if updated < cutoff_fresh {
            continue;
        }
        // Race: an agent can self-delete its own broadcast (or another tick
        // can sweep stale broadcasts) between list above and download below.
        // Both backends translate the 404 into a None return so the
        // missing-blob case is the only one we drop; any other error
        // propagates to the caller so transient SDK/network failures stay
        // visible.
        let Some(raw) = store.download_text(&blob.name).await? else { continue };
        let payload: Value = serde_json::from_str(&raw)?;
        if let Some(cid) = payload.get("consumer_id").and_then(Value::as_str) {
            out.insert(cid.to_string(), payload);
        }
    }

    for name in stale_blobs.into_iter().take(CAPACITY_GC_CAP_PER_TICK) {
        store.delete_blob(&name).await?;
    }
    Ok(out)
}

/// Sum free_slots across consumers (optionally filtered by kind). Python
/// `total_free_by_accel`.
pub fn total_free_by_accel(
    consumers: &BTreeMap<String, Value>,
    kinds: Option<&[&str]>,
) -> BTreeMap<String, i64> {
    let mut totals: BTreeMap<String, i64> = BTreeMap::new();
    for payload in consumers.values() {
        if let Some(kinds) = kinds {
            let kind = payload.get("kind").and_then(Value::as_str).unwrap_or("");
            if !kinds.contains(&kind) {
                continue;
            }
        }
        let Some(slots) = payload.get("free_slots").and_then(Value::as_object) else { continue };
        for (accel, n) in slots {
            *totals.entry(accel.clone()).or_insert(0) += n.as_i64().unwrap_or(0);
        }
    }
    totals
}

/// [(consumer_id, free_vram_gb), ...] sorted descending. Empty if none
/// publish vram. Python `consumers_by_free_vram`.
pub fn consumers_by_free_vram(
    consumers: &BTreeMap<String, Value>,
    kinds: Option<&[&str]>,
) -> Vec<(String, i64)> {
    let mut rows: Vec<(String, i64)> = Vec::new();
    for payload in consumers.values() {
        if let Some(kinds) = kinds {
            let kind = payload.get("kind").and_then(Value::as_str).unwrap_or("");
            if !kinds.contains(&kind) {
                continue;
            }
        }
        let v = payload
            .get("free_vram_gb")
            .and_then(|v| v.as_i64().or_else(|| v.as_f64().map(|f| f as i64)));
        let Some(v) = v else { continue };
        // Python `payload["consumer_id"]`; read_consumer_capacity only emits
        // payloads that carry the key.
        let Some(cid) = payload.get("consumer_id").and_then(Value::as_str) else { continue };
        rows.push((cid.to_string(), v));
    }
    rows.sort_by(|a, b| b.1.cmp(&a.1));
    rows
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::queue::local_file::LocalBackend;
    use std::sync::Arc;

    fn store() -> (tempfile::TempDir, JobStorage) {
        let dir = tempfile::tempdir().unwrap();
        let backend = LocalBackend::new(dir.path().to_str().unwrap()).unwrap();
        (dir, JobStorage::with_backend(Arc::new(backend), "local"))
    }

    #[tokio::test]
    async fn publish_then_read_returns_live_consumers() {
        let (_dir, store) = store();
        let diag = Map::from_iter([
            ("queue_scanned".to_string(), Value::from(12)),
            ("claimed_this_loop".to_string(), Value::from(1)),
        ]);
        publish_capacity(
            &store,
            "local-a",
            "local",
            &BTreeMap::from([("nvidia-l4".to_string(), 1)]),
            Some(20),
            Some(24),
            Some(diag),
        )
        .await
        .unwrap();
        publish_capacity(
            &store,
            "gcp-b",
            "gcp",
            &BTreeMap::from([("nvidia-l4".to_string(), 2), ("nvidia-tesla-a100".to_string(), 1)]),
            None,
            None,
            None,
        )
        .await
        .unwrap();
        // Non-broadcast blobs under the prefix are ignored.
        store.upload_text("capacity/README.txt", "not a broadcast").await.unwrap();

        let live = read_consumer_capacity(&store).await.unwrap();
        assert_eq!(live.len(), 2);
        let a = &live["local-a"];
        assert_eq!(a["kind"], "local");
        assert_eq!(a["free_vram_gb"], 20);
        assert_eq!(a["total_vram_gb"], 24);
        assert_eq!(a["diag"]["queue_scanned"], 12);
        assert_eq!(a["stado_version"], env!("CARGO_PKG_VERSION"));
        assert!(a["published_at"].as_str().unwrap().contains('T'));
        // Consumers that do not publish vram simply lack the key.
        assert!(live["gcp-b"].get("free_vram_gb").is_none());

        // Aggregation helpers.
        let all = total_free_by_accel(&live, None);
        assert_eq!(all, BTreeMap::from([
            ("nvidia-l4".to_string(), 3),
            ("nvidia-tesla-a100".to_string(), 1),
        ]));
        let local_only = total_free_by_accel(&live, Some(&["local"]));
        assert_eq!(local_only, BTreeMap::from([("nvidia-l4".to_string(), 1)]));
        assert_eq!(consumers_by_free_vram(&live, None), vec![("local-a".to_string(), 20)]);
        assert!(consumers_by_free_vram(&live, Some(&["gcp"])).is_empty());
    }

    #[tokio::test]
    async fn stale_broadcasts_are_filtered_but_not_deleted() {
        let (_dir, store) = store();
        publish_capacity(&store, "c1", "local", &BTreeMap::new(), Some(8), None, None)
            .await
            .unwrap();
        // Fresh against the real clock.
        assert_eq!(read_consumer_capacity(&store).await.unwrap().len(), 1);
        // Past the 180s staleness horizon: filtered out, but the blob is
        // retained (it is younger than the 1h GC cutoff).
        let later = Utc::now() + Duration::seconds(CAPACITY_STALE_SECONDS as i64 + 60);
        assert!(read_consumer_capacity_at(&store, later).await.unwrap().is_empty());
        assert_eq!(store.list_paths(CAPACITY_PREFIX, 0).await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn gc_deletes_hour_old_broadcasts_capped_at_200_per_tick() {
        let (_dir, store) = store();
        for i in 0..250 {
            store
                .upload_text(&format!("capacity/c{i:03}.json"), "{\"consumer_id\": \"x\"}")
                .await
                .unwrap();
        }
        // Every blob is >1h old from this clock -> all GC-eligible, and the
        // first 200 are swept this tick.
        let later = Utc::now() + Duration::seconds(CAPACITY_GC_AGE_SECONDS + 60);
        assert!(read_consumer_capacity_at(&store, later).await.unwrap().is_empty());
        assert_eq!(store.list_paths(CAPACITY_PREFIX, 0).await.unwrap().len(), 50);
        // The next tick sweeps the rest.
        assert!(read_consumer_capacity_at(&store, later).await.unwrap().is_empty());
        assert!(store.list_paths(CAPACITY_PREFIX, 0).await.unwrap().is_empty());
    }
}
