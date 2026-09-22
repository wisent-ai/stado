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

use crate::primitives::constants;

use super::storage::JobStorage;
use super::StorageError;

mod publications;
mod readers;
pub mod reservations;

pub use publications::{read_consumer_capacity, read_publications, Publication};
pub use readers::*;
pub use reservations::{Reservation, Reserved};

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
/// Resources measured by one worker at one point in time. `available_*`
/// and `free_*` are what the worker measured; [`publish_capacity`] nets the
/// host's live reservations off them before anyone else reads them.
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

/// The reservation side of one publication: which holds are live on the
/// host, and their sum.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct PublishedReservations {
    pub live: Vec<Reservation>,
    pub reserved: Reserved,
}

impl PublishedReservations {
    /// Read this consumer's live reservations from the store. A store that
    /// cannot be listed publishes no reservations rather than no capacity:
    /// the error is returned so the caller can log it, and an empty set is
    /// the fallback it should publish with.
    pub async fn read(
        store: &JobStorage,
        consumer_id: &str,
        now: DateTime<Utc>,
    ) -> Result<Self, StorageError> {
        let live = reservations::live_for_consumer(store, consumer_id, now).await?;
        let reserved = Reserved::of(&live);
        Ok(Self { live, reserved })
    }
}

/// Whether a `<kind>-<hostname>` publication key names `target`, by the
/// fleet's one hostname-to-target rule ([`crate::targets::Registry::lookup_self`]).
/// The hostname a host publishes is its own, which need not be its registry
/// name, so string equality on the key is wrong on every renamed machine.
pub fn consumer_names_target(
    registry: &crate::targets::Registry,
    target: &crate::targets::ComputeTarget,
    consumer_id: &str,
) -> bool {
    let Some(identity) = consumer_id.strip_prefix(&format!("{}-", target.kind)) else {
        return false;
    };
    registry
        .lookup_self(identity)
        .ok()
        .flatten()
        .is_some_and(|found| found.name == target.name)
}

/// The consumer id `target`'s agent publishes under: the key of its
/// publication when one exists, else the id it would use, derived from its
/// first declared hostname.
pub fn consumer_id_for_target(
    registry: &crate::targets::Registry,
    target: &crate::targets::ComputeTarget,
    publications: &BTreeMap<String, Publication>,
) -> String {
    publications
        .keys()
        .find(|consumer| consumer_names_target(registry, target, consumer))
        .cloned()
        .unwrap_or_else(|| {
            let host = target
                .hostnames
                .first()
                .map(|host| crate::targets::normalize_hostname(host))
                .unwrap_or_else(|| target.name.clone());
            format!("{}-{host}", target.kind)
        })
}

/// Write this worker's current measured resource snapshot.
///
/// `accepting_jobs` is the admission decision consumed by dispatchers. The
/// remaining fields explain it and let GPU placement compare a job's declared
/// needs with live hardware state; none is an operator-set concurrency limit.
///
/// The host's declared memory refusal is applied HERE and not by the caller,
/// because a capacity document has two writers on every host — the agent tick
/// and the heartbeat republisher — and a host that published itself as
/// claimable from one of them while its own `targets[].memory_reclaim`
/// refuses work would be selected for exactly the work it cannot run. That is
/// what happened to charless-mac-mini on 2026-09-06: a machine that could no
/// longer give a runtime its heap kept being selected, and every selection
/// died with `Failed to create CoreCLR, HRESULT: 0x8007000C`. The refusal is
/// declared, never inferred: `refuse_placement` is a registry field, and a
/// host that does not declare it publishes exactly what its caller measured.
///
/// Live reservations are subtracted here for the same reason: both writers
/// pass through this function, so neither can publish a host as free while
/// a Jeden session or a browser task holds it. The measured numbers stay in
/// `diag.measured_*` so an operator can see what the host has and what is
/// held, not only the difference.
pub async fn publish_capacity(
    store: &JobStorage,
    consumer_id: &str,
    kind: &str,
    snapshot: &CapacitySnapshot,
) -> Result<(), StorageError> {
    let memory = crate::providers::local::host_memory::placement_decision();
    let now = Utc::now();
    let mut diag = snapshot.diag.clone();
    let held = match PublishedReservations::read(store, consumer_id, now).await {
        Ok(held) => held,
        Err(error) => {
            diag.insert(
                "reservations_error".into(),
                Value::String(error.to_string()),
            );
            PublishedReservations::default()
        }
    };
    let net_cpu = (snapshot.available_cpu_cores - held.reserved.cpu_cores).max(0);
    let net_ram = snapshot
        .free_ram_gb
        .map(|free| (free - held.reserved.ram_gb).max(0.0));
    let net_vram = (snapshot.free_vram_gb - held.reserved.vram_gb).max(0);
    let reservations_exhausted = !held.live.is_empty()
        && (net_cpu < constants::RESERVATION_MIN_FREE_CORES
            || net_ram.is_some_and(|free| free < constants::RESERVATION_MIN_FREE_RAM_GB));
    let mut accepting = snapshot.accepting_jobs && memory.reason.is_none();
    if accepting && reservations_exhausted {
        accepting = false;
        diag.insert(
            "admission_reason".into(),
            Value::from("reservations_exhausted"),
        );
    }
    diag.insert(
        "measured_available_cpu_cores".into(),
        Value::from(snapshot.available_cpu_cores),
    );
    if let Some(free) = snapshot.free_ram_gb {
        diag.insert("measured_free_ram_gb".into(), Value::from(free));
    }
    diag.insert(
        "measured_free_vram_gb".into(),
        Value::from(snapshot.free_vram_gb),
    );
    let mut payload = Map::new();
    payload.insert("consumer_id".into(), Value::String(consumer_id.to_string()));
    payload.insert("kind".into(), Value::String(kind.to_string()));
    payload.insert("published_at".into(), Value::String(now.to_rfc3339()));
    payload.insert("accepting_jobs".into(), Value::from(accepting));
    payload.insert(
        "running_jobs".into(),
        Value::from(snapshot.running_jobs as i64),
    );
    payload.insert(
        "running_workloads".into(),
        Value::from(held.live.len() as i64),
    );
    payload.insert(
        "total_cpu_cores".into(),
        Value::from(snapshot.total_cpu_cores),
    );
    payload.insert("available_cpu_cores".into(), Value::from(net_cpu));
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
    payload.insert("free_vram_gb".into(), Value::from(net_vram));
    payload.insert("total_vram_gb".into(), Value::from(snapshot.total_vram_gb));
    if let Some(value) = net_ram {
        payload.insert("free_ram_gb".into(), Value::from(value));
    }
    if let Some(value) = snapshot.total_ram_gb {
        payload.insert("total_ram_gb".into(), Value::from(value));
    }
    payload.insert("reserved".into(), serde_json::to_value(held.reserved)?);
    payload.insert(
        "reservations".into(),
        Value::Array(held.live.iter().map(Reservation::published).collect()),
    );
    diag.append(&mut memory.diagnostics());
    payload.insert("diag".into(), Value::Object(diag));
    payload.insert(
        "stado_version".into(),
        Value::String(env!("CARGO_PKG_VERSION").to_string()),
    );
    let body = super::python_json_dumps(&Value::Object(payload))?;
    store
        .upload_text(&format!("{CAPACITY_PREFIX}{consumer_id}.json"), &body)
        .await
}
