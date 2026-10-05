//! Live resource broadcasts from workers.
//!
//! Each worker publishes whether it can accept another job together with the
//! measured resources behind that decision. Nothing in this document is a
//! configured concurrency allowance: CPU availability comes from the host's
//! logical processors and current load, RAM and VRAM come from the operating
//! system and accelerator driver, and accelerator counts are derived from the
//! memory each workload class requires.
//!
//! Each `<bucket>/capacity/<consumer_id>.json` object identifies its worker
//! with `consumer_id` and `kind`, reports `accepting_jobs` and `running_jobs`,
//! and carries `available_cpu_cores`, per-type `available_accelerators`,
//! `free_ram_gb` and `free_vram_gb`. `published_at` is an RFC3339 UTC timestamp,
//! and `next_by` is the time by which its publisher promises the next one: its
//! own republish period plus how long its last publication took to write. A
//! reader counts a publication only until its `next_by`, so readers outside
//! Stado (dashboards) judge liveness by the publisher's own promise, never by a
//! window of their own.
//!
//! A publication written before `next_by` existed carried the window its
//! publisher applied as `stale_after_seconds`; that row is judged by the window
//! it states. A row that states neither is not live: nothing says when its
//! author meant to speak again.
//!
//! Capacity discovery requires metadata-bearing object listings, supported
//! by every Rust `BlobBackend`; it does not depend on a provider-specific
//! SDK handle.

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
/// The republish period of this process's capacity publisher, declared once
/// by the agent that publishes ([`declare_publisher_cadence`]). A process that
/// never declares one publishes rows without `next_by`, which no reader
/// counts as live.
static PUBLISHER_CADENCE: std::sync::OnceLock<std::time::Duration> = std::sync::OnceLock::new();
/// How long this process's last capacity publication took to write, in
/// milliseconds: the part of the promise the period alone does not cover.
static LAST_PUBLISH_MILLIS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Declare the period at which this process republishes its capacity. The
/// agent calls this with its own poll period before its first publication.
pub fn declare_publisher_cadence(period: std::time::Duration) {
    let _ = PUBLISHER_CADENCE.set(period);
}

/// When `payload`'s author promised its next publication: its own `next_by`,
/// or, for a row from before that field, `published_at` (or the object's own
/// timestamp, `stamp`) plus the `stale_after_seconds` it wrote. `None` when
/// the row states neither.
pub fn next_publication_by(payload: &Value, stamp: Option<DateTime<Utc>>) -> Option<DateTime<Utc>> {
    if let Some(next) = payload
        .get("next_by")
        .and_then(Value::as_str)
        .and_then(|text| DateTime::parse_from_rfc3339(text).ok())
    {
        return Some(next.with_timezone(&Utc));
    }
    let window = payload.get("stale_after_seconds").and_then(Value::as_i64)?;
    Some(stamp? + Duration::seconds(window))
}

/// The instant `payload` says it was published, else the object's own time.
pub fn published_stamp(
    payload: &Value,
    object_time: Option<DateTime<Utc>>,
) -> Option<DateTime<Utc>> {
    payload
        .get("published_at")
        .and_then(Value::as_str)
        .and_then(|text| DateTime::parse_from_rfc3339(text).ok())
        .map(|stamp| stamp.with_timezone(&Utc))
        .or(object_time)
}

/// Whether `payload` is still within the promise its author made.
pub fn publication_live(payload: &Value, stamp: Option<DateTime<Utc>>, now: DateTime<Utc>) -> bool {
    next_publication_by(payload, stamp).is_some_and(|by| now <= by)
}

/// How often `payload`'s publisher said it publishes: the span from its
/// `published_at` to its `next_by`, or the `stale_after_seconds` an older
/// publisher stated. `None` when it states neither. A holder whose work this
/// publisher subtracts renews on this period: the publisher reads no more
/// often than it publishes.
pub fn publication_period(payload: &Value) -> Option<std::time::Duration> {
    let published = published_stamp(payload, None);
    let span = match (published, next_publication_by(payload, published)) {
        (Some(published), Some(next)) => next - published,
        _ => return None,
    };
    span.to_std().ok().filter(|period| !period.is_zero())
}

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
/// The host's memory readings ride in `diag` beside the measured numbers;
/// they explain the host and never refuse work.
///
/// Live reservations are subtracted here because a capacity document has two
/// writers on every host — the agent tick and the heartbeat republisher — and
/// both pass through this function, so neither can publish a host as free while
/// a Jeden session or a browser task holds it. The measured numbers stay in
/// `diag.measured_*` so an operator can see what the host has and what is
/// held, not only the difference.
pub async fn publish_capacity(
    store: &JobStorage,
    consumer_id: &str,
    kind: &str,
    snapshot: &CapacitySnapshot,
) -> Result<(), StorageError> {
    let mut diag = snapshot.diag.clone();
    diag.extend(crate::providers::local::host_memory::publication_fields(
        &crate::providers::local::host_memory::read_host_memory(),
    ));
    let now = Utc::now();
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
    let mut accepting = snapshot.accepting_jobs;
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
    if let Some(period) = PUBLISHER_CADENCE.get() {
        let write = std::time::Duration::from_millis(
            LAST_PUBLISH_MILLIS.load(std::sync::atomic::Ordering::Relaxed),
        );
        let next_by = now + Duration::from_std(*period + write).unwrap_or(Duration::MAX);
        payload.insert("next_by".into(), Value::String(next_by.to_rfc3339()));
    }
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
    // Names only, never values: the `item#field` references a job may project
    // here, so a coordinator does not pin a job this agent cannot resolve. Read
    // as the config holds them now, so a secret enrolled after this agent
    // started is offered without a restart.
    payload.insert(
        "secret_fields".into(),
        Value::from(crate::config::agent_skarbiec_secret_fields_now()),
    );
    payload.insert("diag".into(), Value::Object(diag));
    payload.insert(
        "stado_version".into(),
        Value::String(env!("CARGO_PKG_VERSION").to_string()),
    );
    let body = super::python_json_dumps(&Value::Object(payload))?;
    let started = std::time::Instant::now();
    let written = store
        .upload_text(&format!("{CAPACITY_PREFIX}{consumer_id}.json"), &body)
        .await;
    if written.is_ok() {
        LAST_PUBLISH_MILLIS.store(
            u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
            std::sync::atomic::Ordering::Relaxed,
        );
    }
    written
}
