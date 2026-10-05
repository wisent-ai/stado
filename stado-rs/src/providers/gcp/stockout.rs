//! Shared marks of GCE zones that answered ZONE_RESOURCE_POOL_EXHAUSTED
//! (stockout) and of (region, accelerator) pairs that answered
//! QUOTA_EXCEEDED.
//!
//! Concurrent coordinator processes need shared exhaustion observations.
//! A process-local map makes each new process repeat provider calls against
//! the same exhausted zones, so the marks live in the job store:
//! state/stockout_zones.json and state/quota_exceeded.json, each a map from
//! key to the epoch second the exhaustion was observed.
//!
//! A mark is not a window. It holds until an event says the place has room
//! again: a VM created in the zone clears that zone's stockout and its
//! region's quota mark for that accelerator, and a VM deleted in a region
//! clears every quota mark of that region, because the deletion returned
//! quota. A mark that no event clears is retried by rotation: each create
//! call tries every unmarked zone, then the one zone whose mark is oldest.
//! Every exhausted place is retried in turn, at most one per call, and no
//! time decides when.

use std::collections::BTreeMap;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::queue::{JobStorage, StorageError};

/// Zones that answered a stockout, keyed by zone.
pub const STOCKOUT_BLOB: &str = "state/stockout_zones.json";
/// Regions that answered a quota refusal, keyed by `region:accelerator`.
pub const QUOTA_BLOB: &str = "state/quota_exceeded.json";

/// Both mark maps as one create call read them.
#[derive(Debug, Default, Clone)]
pub struct Exhaustion {
    pub zones: BTreeMap<String, f64>,
    pub quotas: BTreeMap<String, f64>,
}

impl Exhaustion {
    /// When `zone` was last seen exhausted for `accel`: the later of its
    /// stockout mark and its region's quota mark, or None when it has none.
    pub fn marked_at(&self, zone: &str, region: &str, accel: &str) -> Option<f64> {
        let stockout = self.zones.get(zone).copied();
        let quota = if accel.is_empty() {
            None
        } else {
            self.quotas.get(&quota_key(region, accel)).copied()
        };
        match (stockout, quota) {
            (Some(a), Some(b)) => Some(a.max(b)),
            (a, b) => a.or(b),
        }
    }

    /// The zones one create call tries, in order: every unmarked zone in the
    /// configured order, then the single marked zone whose mark is oldest.
    /// The second list is the marked zones this call skips, with their marks.
    pub fn attempt_order(
        &self,
        zones: &[String],
        region_of: impl Fn(&str) -> String,
        accel: &str,
    ) -> (Vec<String>, Vec<(String, f64)>) {
        let mut open = Vec::new();
        let mut marked: Vec<(String, f64)> = Vec::new();
        for zone in zones {
            match self.marked_at(zone, &region_of(zone), accel) {
                None => open.push(zone.clone()),
                Some(at) => marked.push((zone.clone(), at)),
            }
        }
        marked.sort_by(|a, b| a.1.total_cmp(&b.1));
        let mut skipped = marked.into_iter();
        if let Some((retry, _)) = skipped.next() {
            open.push(retry);
        }
        (open, skipped.collect())
    }
}

fn quota_key(region: &str, accel: &str) -> String {
    format!("{region}:{accel}")
}

/// Epoch seconds as float.
fn now_epoch() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

/// `float(v)` on a JSON scalar: numbers pass, numeric strings parse.
fn json_float(value: &serde_json::Value) -> Option<f64> {
    match value {
        serde_json::Value::Number(n) => n.as_f64(),
        serde_json::Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
}

/// Read one mark map from the store, every time it is asked for.
///
/// A missing blob means nothing was marked yet. A corrupt or non-object blob
/// reads as empty: the next mark overwrites it, and a corrupted state file
/// must not stop the autoscaler. Any other storage error propagates.
async fn load(store: &JobStorage, blob: &str) -> Result<BTreeMap<String, f64>, StorageError> {
    Ok(match store.download_text(blob).await? {
        None => BTreeMap::new(),
        Some(text) => match serde_json::from_str::<serde_json::Value>(&text) {
            Ok(serde_json::Value::Object(obj)) => obj
                .iter()
                .filter_map(|(k, v)| json_float(v).map(|f| (k.clone(), f)))
                .collect(),
            _ => BTreeMap::new(),
        },
    })
}

/// Write one mark map. A failed write does not abort the create call; it is
/// reported, and the next call simply does not see the mark.
async fn save(store: &JobStorage, blob: &str, map: &BTreeMap<String, f64>) {
    let json = serde_json::to_string(map).unwrap_or_else(|_| "{}".into());
    if let Err(err) = store.upload_text(blob, &json).await {
        tracing::warn!(blob, %err, "gcp exhaustion mark was not written");
    }
}

/// Rewrite one map with `edit` applied, writing only when it changed.
async fn update(
    store: &JobStorage,
    blob: &str,
    edit: impl FnOnce(&mut BTreeMap<String, f64>),
) -> Result<(), StorageError> {
    let before = load(store, blob).await?;
    let mut after = before.clone();
    edit(&mut after);
    if after != before {
        save(store, blob, &after).await;
    }
    Ok(())
}

/// Both mark maps, read now.
pub async fn load_exhaustion(store: &JobStorage) -> Result<Exhaustion, StorageError> {
    Ok(Exhaustion {
        zones: load(store, STOCKOUT_BLOB).await?,
        quotas: load(store, QUOTA_BLOB).await?,
    })
}

/// Record that `zone` answered a stockout now.
pub async fn mark_zone_stockout(store: &JobStorage, zone: &str) -> Result<(), StorageError> {
    let now = now_epoch();
    update(store, STOCKOUT_BLOB, |map| {
        map.insert(zone.to_string(), now);
    })
    .await
}

/// Record that `region` refused `accel` for quota now. Keys look like
/// "us-central1:nvidia-tesla-a100" so different accelerator quotas in the
/// same region are marked independently.
pub async fn mark_region_quota_exceeded(
    store: &JobStorage,
    region: &str,
    accel: &str,
) -> Result<(), StorageError> {
    let now = now_epoch();
    let key = quota_key(region, accel);
    update(store, QUOTA_BLOB, |map| {
        map.insert(key, now);
    })
    .await
}

/// A VM was created in `zone`: the zone has room, and its region has quota
/// for `accel`.
pub async fn clear_after_create(
    store: &JobStorage,
    zone: &str,
    region: &str,
    accel: &str,
) -> Result<(), StorageError> {
    update(store, STOCKOUT_BLOB, |map| {
        map.remove(zone);
    })
    .await?;
    if accel.is_empty() {
        return Ok(());
    }
    let key = quota_key(region, accel);
    update(store, QUOTA_BLOB, |map| {
        map.remove(&key);
    })
    .await
}

/// A VM was deleted in `region`: the quota it held is free again for every
/// accelerator marked there.
pub async fn clear_after_delete(store: &JobStorage, region: &str) -> Result<(), StorageError> {
    let prefix = quota_key(region, "");
    update(store, QUOTA_BLOB, |map| {
        map.retain(|key, _| !key.starts_with(&prefix));
    })
    .await
}
