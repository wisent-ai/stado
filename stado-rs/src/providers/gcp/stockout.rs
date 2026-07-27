//! Cross-instance caches of GCE zones currently returning
//! ZONE_RESOURCE_POOL_EXHAUSTED (stockout) and regions returning
//! QUOTA_EXCEEDED.
//!
//! Port of `stado/providers/gcp/stockout.py`.
//!
//! With Cloud Function maxScale=100 and ticks lasting longer than the
//! 3-minute cron, the function spawns multiple parallel instances, each
//! with its own process state. A process-local map was insufficient: every
//! fresh instance re-discovered the same exhausted zones at ~30s per
//! op.result() call, blowing past the 540s tick timeout. Confirmed in
//! production logs 01:46-01:48Z 2026-05-15: us-central1-c stocked out twice
//! in 8 seconds across two parallel function instances.
//!
//! This module persists the stockout map to gs://<bucket>/state/
//! stockout_zones.json (and the quota map to state/quota_exceeded.json) so
//! every Cloud Function instance shares what's exhausted right now. Each
//! blob is read at most every [`LOCAL_CACHE_TTL_S`] seconds (in-process
//! cache) and written only on exhaustion detection (rare).
//!
//! Deviation: Python's `_stockout_blob()` returns None when the JobStorage
//! has no GCS SDK bucket (non-GCS backends), disabling the caches there.
//! The Rust [`JobStorage`] always has a working backend, so the caches stay
//! functional on the local backend too (dev/test deployments) — the blob
//! layout and TTL semantics are unchanged.

use std::collections::BTreeMap;
use std::sync::{LazyLock, RwLock};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::queue::{JobStorage, StorageError};

/// Python `STOCKOUT_TTL_S`.
pub const STOCKOUT_TTL_S: f64 = 300.0;
/// Python `STOCKOUT_BLOB`.
pub const STOCKOUT_BLOB: &str = "state/stockout_zones.json";
/// Python `_LOCAL_CACHE_TTL_S`.
pub const LOCAL_CACHE_TTL_S: f64 = 10.0;
/// Python `QUOTA_TTL_S`.
pub const QUOTA_TTL_S: f64 = 60.0;
/// Python `QUOTA_BLOB`.
pub const QUOTA_BLOB: &str = "state/quota_exceeded.json";

/// In-process mirror of one GCS blob (Python's module-level `_local_cache`
/// / `_quota_cache` plus their `built_at` stamps). `std::sync::RwLock` —
/// the lock is never held across an await.
#[derive(Default)]
struct LocalCache {
    map: BTreeMap<String, f64>,
    built_at: f64,
}

static STOCKOUT_CACHE: LazyLock<RwLock<LocalCache>> = LazyLock::new(RwLock::default);
static QUOTA_CACHE: LazyLock<RwLock<LocalCache>> = LazyLock::new(RwLock::default);

/// Python `time.time()` (epoch seconds as float).
fn now_epoch() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

/// Python `float(v)` on a JSON scalar: numbers pass, numeric strings parse.
fn py_float(value: &serde_json::Value) -> Option<f64> {
    match value {
        serde_json::Value::Number(n) => n.as_f64(),
        serde_json::Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
}

/// Python `_load_stockouts` / `_load_quota` (same shape, two blobs).
///
/// Refreshes the in-process cache at most every [`LOCAL_CACHE_TTL_S`]
/// seconds. A missing blob means the new-cluster state (nothing logged
/// yet); return an empty map. A corrupt (JSONDecodeError) or non-dict blob
/// also returns empty — the recovery path is to overwrite the blob on the
/// next stockout, and a corrupted state file should not crash the
/// autoscaler. Any other storage error propagates so the operator sees it.
///
/// Note the Python freshness check requires the cache to be non-empty —
/// an empty in-process cache always re-reads the blob.
async fn load(
    store: &JobStorage,
    cache: &RwLock<LocalCache>,
    blob: &str,
) -> Result<BTreeMap<String, f64>, StorageError> {
    let now = now_epoch();
    {
        let local = cache.read().expect("stockout cache lock");
        if now - local.built_at < LOCAL_CACHE_TTL_S && !local.map.is_empty() {
            return Ok(local.map.clone());
        }
    }
    let parsed: BTreeMap<String, f64> = match store.download_text(blob).await? {
        None => BTreeMap::new(),
        Some(text) => match serde_json::from_str::<serde_json::Value>(&text) {
            Ok(serde_json::Value::Object(obj)) => obj
                .iter()
                .filter_map(|(k, v)| py_float(v).map(|f| (k.clone(), f)))
                .collect(),
            // Corrupt JSON or a non-dict payload: treat as empty (see docs).
            _ => BTreeMap::new(),
        },
    };
    {
        let mut local = cache.write().expect("stockout cache lock");
        local.map = parsed.clone();
        local.built_at = now;
    }
    Ok(parsed)
}

/// Python `_save_stockouts` and the save half of `mark_region_quota_exceeded`.
///
/// A network blip during a cache write must not crash the entire tick —
/// the in-process cache still has the marker, and the next tick will retry
/// the write. Confirmed live 04:07Z 2026-05-15: SSL EOF on the
/// upload_from_string raised through create_instance and crashed the whole
/// monitor_jobs handler. Errors are swallowed here.
async fn save(store: &JobStorage, blob: &str, map: &BTreeMap<String, f64>) {
    let json = serde_json::to_string(map).unwrap_or_else(|_| "{}".into());
    let _ = store.upload_text(blob, &json).await;
}

/// The shared mark path: load, stamp `key = now`, prune entries older than
/// 2× the TTL, best-effort save, and update the in-process cache (Python
/// `mark_zone_stockout` / `mark_region_quota_exceeded`).
async fn mark(
    store: &JobStorage,
    cache: &RwLock<LocalCache>,
    blob: &str,
    ttl_s: f64,
    key: &str,
) -> Result<(), StorageError> {
    let mut map = load(store, cache, blob).await?;
    let now = now_epoch();
    map.insert(key.to_string(), now);
    map.retain(|_, ts| now - *ts < 2.0 * ttl_s);
    save(store, blob, &map).await;
    let mut local = cache.write().expect("stockout cache lock");
    local.map = map;
    local.built_at = now;
    Ok(())
}

/// The shared "recently exhausted?" probe (Python
/// `zone_recently_stocked_out` / `region_recently_quota_exceeded`).
async fn recently(
    store: &JobStorage,
    cache: &RwLock<LocalCache>,
    blob: &str,
    ttl_s: f64,
    key: &str,
) -> Result<bool, StorageError> {
    let map = load(store, cache, blob).await?;
    let Some(ts) = map.get(key) else { return Ok(false) };
    Ok(now_epoch() - ts < ttl_s)
}

/// Python `zone_recently_stocked_out`.
pub async fn zone_recently_stocked_out(
    store: &JobStorage,
    zone: &str,
) -> Result<bool, StorageError> {
    recently(store, &STOCKOUT_CACHE, STOCKOUT_BLOB, STOCKOUT_TTL_S, zone).await
}

/// Python `mark_zone_stockout`.
pub async fn mark_zone_stockout(store: &JobStorage, zone: &str) -> Result<(), StorageError> {
    mark(store, &STOCKOUT_CACHE, STOCKOUT_BLOB, STOCKOUT_TTL_S, zone).await
}

// Region-level quota cache: keys look like "us-central1:nvidia-tesla-a100"
// so different accel quotas in the same region cache independently. Quota
// resets when other VMs terminate, but within a tick window the same
// region's quota stays exhausted, so 60s is enough TTL to skip retries.

/// Python `region_recently_quota_exceeded`.
pub async fn region_recently_quota_exceeded(
    store: &JobStorage,
    region: &str,
    accel: &str,
) -> Result<bool, StorageError> {
    let key = format!("{region}:{accel}");
    recently(store, &QUOTA_CACHE, QUOTA_BLOB, QUOTA_TTL_S, &key).await
}

/// Python `mark_region_quota_exceeded`.
pub async fn mark_region_quota_exceeded(
    store: &JobStorage,
    region: &str,
    accel: &str,
) -> Result<(), StorageError> {
    let key = format!("{region}:{accel}");
    mark(store, &QUOTA_CACHE, QUOTA_BLOB, QUOTA_TTL_S, &key).await
}

/// Serialize the local caches and take a global lock so the TTL tests can
/// run against shared process state without racing each other. Async
/// (tokio) mutex: the guard is held across the test's API calls.
#[cfg(test)]
pub(crate) async fn test_lock() -> tokio::sync::MutexGuard<'static, ()> {
    static LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
    let guard = LOCK.lock().await;
    for cache in [&STOCKOUT_CACHE, &QUOTA_CACHE] {
        let mut local = cache.write().expect("stockout cache lock");
        *local = LocalCache::default();
    }
    guard
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
    async fn empty_store_reports_no_stockout_and_no_quota() {
        let _guard = test_lock().await;
        let (_dir, store) = store();
        assert!(!zone_recently_stocked_out(&store, "zz-test-a").await.unwrap());
        assert!(!region_recently_quota_exceeded(&store, "zz-test", "nvidia-tesla-t4").await.unwrap());
    }

    #[tokio::test]
    async fn mark_persists_blob_and_updates_in_process_cache() {
        let _guard = test_lock().await;
        let (_dir, store) = store();
        mark_zone_stockout(&store, "zz-test-b").await.unwrap();
        assert!(zone_recently_stocked_out(&store, "zz-test-b").await.unwrap());
        assert!(!zone_recently_stocked_out(&store, "zz-test-c").await.unwrap());

        // The blob carries the marker as a JSON float timestamp.
        let text = store.download_text(STOCKOUT_BLOB).await.unwrap().unwrap();
        let doc: serde_json::Value = serde_json::from_str(&text).unwrap();
        let ts = doc["zz-test-b"].as_f64().unwrap();
        assert!(now_epoch() - ts < 5.0, "stale timestamp: {ts}");

        // In-process cache: delete the blob — within LOCAL_CACHE_TTL_S the
        // marker is still served from memory without a re-read.
        store.delete_blob(STOCKOUT_BLOB).await.unwrap();
        assert!(zone_recently_stocked_out(&store, "zz-test-b").await.unwrap());
    }

    #[tokio::test]
    async fn ttl_expiry_reads_from_blob() {
        let _guard = test_lock().await;
        let (_dir, store) = store();
        let now = now_epoch();
        // Stockout TTL 300s: 400s old is expired, 100s old is live.
        store
            .upload_text(
                STOCKOUT_BLOB,
                &serde_json::json!({"zz-old": now - 400.0, "zz-fresh": now - 100.0}).to_string(),
            )
            .await
            .unwrap();
        assert!(!zone_recently_stocked_out(&store, "zz-old").await.unwrap());
        assert!(zone_recently_stocked_out(&store, "zz-fresh").await.unwrap());

        // Quota TTL 60s: 70s old is expired, 30s old is live.
        store
            .upload_text(
                QUOTA_BLOB,
                &serde_json::json!({
                    format!("zz-reg:nvidia-tesla-t4"): now - 70.0,
                    format!("zz-reg:nvidia-l4"): now - 30.0,
                })
                .to_string(),
            )
            .await
            .unwrap();
        assert!(!region_recently_quota_exceeded(&store, "zz-reg", "nvidia-tesla-t4").await.unwrap());
        assert!(region_recently_quota_exceeded(&store, "zz-reg", "nvidia-l4").await.unwrap());
    }

    #[tokio::test]
    async fn corrupt_or_non_dict_blob_reads_as_empty() {
        let _guard = test_lock().await;
        let (_dir, store) = store();
        store.upload_text(STOCKOUT_BLOB, "{not json").await.unwrap();
        assert!(!zone_recently_stocked_out(&store, "zz-x").await.unwrap());
        store.upload_text(STOCKOUT_BLOB, "[1, 2, 3]").await.unwrap();
        // The 10s in-process cache held the corrupt-read empty map, which is
        // itself empty — an empty cache re-reads the blob (Python parity).
        assert!(!zone_recently_stocked_out(&store, "zz-x").await.unwrap());
    }

    #[tokio::test]
    async fn mark_prunes_entries_older_than_double_ttl() {
        let _guard = test_lock().await;
        let (_dir, store) = store();
        let now = now_epoch();
        store
            .upload_text(
                QUOTA_BLOB,
                &serde_json::json!({
                    "zz-r:nvidia-tesla-t4": now - 119.0,   // kept (< 2*60)
                    "zz-r:nvidia-l4": now - 121.0,         // pruned (> 2*60)
                })
                .to_string(),
            )
            .await
            .unwrap();
        mark_region_quota_exceeded(&store, "zz-r", "nvidia-a100").await.unwrap();
        let text = store.download_text(QUOTA_BLOB).await.unwrap().unwrap();
        let doc: serde_json::Value = serde_json::from_str(&text).unwrap();
        let obj = doc.as_object().unwrap();
        assert!(obj.contains_key("zz-r:nvidia-tesla-t4"), "{text}");
        assert!(obj.contains_key("zz-r:nvidia-a100"), "{text}");
        assert!(!obj.contains_key("zz-r:nvidia-l4"), "{text}");
    }

    #[tokio::test]
    async fn quota_key_is_region_colon_accel() {
        let _guard = test_lock().await;
        let (_dir, store) = store();
        mark_region_quota_exceeded(&store, "us-central1", "nvidia-tesla-a100").await.unwrap();
        let text = store.download_text(QUOTA_BLOB).await.unwrap().unwrap();
        let doc: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert!(doc.as_object().unwrap().contains_key("us-central1:nvidia-tesla-a100"), "{text}");
        // A different accel in the same region caches independently.
        assert!(!region_recently_quota_exceeded(&store, "us-central1", "nvidia-l4").await.unwrap());
        assert!(region_recently_quota_exceeded(&store, "us-central1", "nvidia-tesla-a100").await.unwrap());
    }
}
