//! Live resource broadcasts from workers.
//!
//! Each worker publishes whether it can accept another job together with the
//! measured resources behind that decision. Nothing in this document is a
//! configured concurrency allowance: CPU availability comes from the host's
//! logical processors and current load, RAM and VRAM come from the operating
//! system and accelerator driver, and accelerator counts are derived from the
//! memory each workload class requires.
//!
//! Scheme:
//!   <bucket>/capacity/<consumer_id>.json
//!   {
//!     "consumer_id": "local-rtx-pro-6000-1",
//!     "kind": "local",
//!     "accepting_jobs": true,
//!     "running_jobs": 2,
//!     "available_cpu_cores": 20,
//!     "available_accelerators": {"nvidia-tesla-a100": 1},
//!     "free_ram_gb": 81.4,
//!     "free_vram_gb": 63,
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
/// Resources measured by one worker at one point in time.
#[derive(Debug, Clone, PartialEq)]
pub struct CapacitySnapshot {
    pub accepting_jobs: bool,
    pub running_jobs: usize,
    pub total_cpu_cores: i64,
    pub available_cpu_cores: i64,
    pub available_accelerators: BTreeMap<String, i64>,
    pub free_ram_gb: Option<f64>,
    pub total_ram_gb: Option<f64>,
    pub free_vram_gb: i64,
    pub total_vram_gb: i64,
    pub diag: Map<String, Value>,
}

/// Write this worker's current measured resource snapshot.
///
/// `accepting_jobs` is the admission decision consumed by dispatchers. The
/// remaining fields explain it and let GPU placement compare a job's declared
/// needs with live hardware state; none is an operator-set concurrency limit.
pub async fn publish_capacity(
    store: &JobStorage,
    consumer_id: &str,
    kind: &str,
    snapshot: &CapacitySnapshot,
) -> Result<(), StorageError> {
    let mut payload = Map::new();
    payload.insert("consumer_id".into(), Value::String(consumer_id.to_string()));
    payload.insert("kind".into(), Value::String(kind.to_string()));
    payload.insert(
        "published_at".into(),
        Value::String(Utc::now().to_rfc3339()),
    );
    payload.insert(
        "accepting_jobs".into(),
        Value::from(snapshot.accepting_jobs),
    );
    payload.insert(
        "running_jobs".into(),
        Value::from(snapshot.running_jobs as i64),
    );
    payload.insert(
        "total_cpu_cores".into(),
        Value::from(snapshot.total_cpu_cores),
    );
    payload.insert(
        "available_cpu_cores".into(),
        Value::from(snapshot.available_cpu_cores),
    );
    payload.insert(
        "available_accelerators".into(),
        Value::Object(
            snapshot
                .available_accelerators
                .iter()
                .map(|(accelerator, count)| (accelerator.clone(), Value::from(*count)))
                .collect(),
        ),
    );
    payload.insert("free_vram_gb".into(), Value::from(snapshot.free_vram_gb));
    payload.insert("total_vram_gb".into(), Value::from(snapshot.total_vram_gb));
    if let Some(value) = snapshot.free_ram_gb {
        payload.insert("free_ram_gb".into(), Value::from(value));
    }
    if let Some(value) = snapshot.total_ram_gb {
        payload.insert("total_ram_gb".into(), Value::from(value));
    }
    payload.insert("diag".into(), Value::Object(snapshot.diag.clone()));
    payload.insert(
        "stado_version".into(),
        Value::String(env!("CARGO_PKG_VERSION").to_string()),
    );
    let body = super::python_json_dumps(&Value::Object(payload))?;
    store
        .upload_text(&format!("{CAPACITY_PREFIX}{consumer_id}.json"), &body)
        .await
}

/// One consumer's publication and the instant it says it was made.
///
/// Carries the stamp separately from the payload because every consumer of a
/// publication asks the same second question — how old is this — and reading
/// `published_at` out of the body at each site is how two surfaces end up
/// disagreeing about whether a row is stale.
#[derive(Debug, Clone, PartialEq)]
pub struct Publication {
    /// The row exactly as its author wrote it.
    pub payload: Value,
    /// `published_at` from the body, falling back to the object's own
    /// timestamp for a body that predates the field or carries an
    /// unparseable one, so a row can never be reported as ageless.
    pub stamp: Option<DateTime<Utc>>,
}

impl Publication {
    /// Seconds since this row was published, or `None` when neither the body
    /// nor the object could say when that was.
    pub fn age_seconds(&self, now: DateTime<Utc>) -> Option<i64> {
        self.stamp.map(|stamp| (now - stamp).num_seconds())
    }

    /// Past [`CAPACITY_STALE_SECONDS`], the horizon every live-capacity
    /// reader in the fleet filters on. An undateable row is NOT stale: it is
    /// a row of unknown age, and calling it stale would invent a fact.
    pub fn stale(&self, now: DateTime<Utc>) -> bool {
        self.age_seconds(now)
            .is_some_and(|age| age > CAPACITY_STALE_SECONDS as i64)
    }
}

/// Return {consumer_id: publication} for EVERY row under
/// [`CAPACITY_PREFIX`], stale ones included, deleting nothing.
///
/// The reader for reports, never for a tick. [`read_consumer_capacity`] is
/// the scheduler's reader and is wrong for anything that has to explain a
/// silent fleet twice over: it drops every row past the staleness horizon,
/// and it DELETES every row past the GC horizon — so an operator command
/// built on it destroys the evidence that a host went quiet an hour ago and
/// then reports that the host never said anything at all.
///
/// The cost that justified the GC-ing reader's metadata prefilter does not
/// apply here: that was a 60s Cloud Function tick against 1900+ accumulated
/// rows, and the tick still runs that reader and still collects them. A
/// report runs once, on an operator's keystroke, against whatever the tick
/// has left.
pub async fn read_publications(
    store: &JobStorage,
) -> Result<BTreeMap<String, Publication>, StorageError> {
    let mut rows: BTreeMap<String, Publication> = BTreeMap::new();
    for blob in store.list_blobs_with_meta(CAPACITY_PREFIX).await? {
        let Some(stem) = blob
            .name
            .strip_prefix(CAPACITY_PREFIX)
            .and_then(|name| name.strip_suffix(".json"))
        else {
            continue;
        };
        // Race: an agent can self-delete its own broadcast, or a scheduler
        // tick can sweep it, between the listing above and the download
        // below. A missing blob is the one case dropped; every other error
        // propagates so a broken store never reads as a quiet fleet.
        let Some(raw) = store.download_text(&blob.name).await? else {
            continue;
        };
        let payload: Value = serde_json::from_str(&raw)?;
        let consumer_id = payload
            .get("consumer_id")
            .and_then(Value::as_str)
            .unwrap_or(stem)
            .to_string();
        let stamp = payload
            .get("published_at")
            .and_then(Value::as_str)
            .and_then(|text| DateTime::parse_from_rfc3339(text).ok())
            .map(|stamp| stamp.with_timezone(&Utc))
            .or(blob.updated);
        rows.insert(consumer_id, Publication { payload, stamp });
    }
    Ok(rows)
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
        let Some(updated) = blob.updated else {
            continue;
        };
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
        let Some(raw) = store.download_text(&blob.name).await? else {
            continue;
        };
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

/// Sum available accelerator placements across accepting workers, optionally
/// filtered by worker kind.
pub fn total_available_accelerators(
    consumers: &BTreeMap<String, Value>,
    kinds: Option<&[&str]>,
) -> BTreeMap<String, i64> {
    let mut totals: BTreeMap<String, i64> = BTreeMap::new();
    for payload in consumers.values() {
        if payload.get("accepting_jobs").and_then(Value::as_bool) != Some(true) {
            continue;
        }
        if let Some(kinds) = kinds {
            let kind = payload.get("kind").and_then(Value::as_str).unwrap_or("");
            if !kinds.contains(&kind) {
                continue;
            }
        }
        let Some(available) = payload
            .get("available_accelerators")
            .and_then(Value::as_object)
        else {
            continue;
        };
        for (accelerator, count) in available {
            *totals.entry(accelerator.clone()).or_insert(0) += count.as_i64().unwrap_or_default();
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
        let Some(cid) = payload.get("consumer_id").and_then(Value::as_str) else {
            continue;
        };
        rows.push((cid.to_string(), v));
    }
    rows.sort_by_key(|row| std::cmp::Reverse(row.1));
    rows
}
