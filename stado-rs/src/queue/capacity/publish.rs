//! Writing this worker's capacity publication: its measured resources net of
//! the host's live reservations, and the promise of when it speaks next.

use chrono::{Duration, Utc};
use serde_json::{Map, Value};

use super::super::storage::JobStorage;
use super::super::StorageError;
use super::{CapacitySnapshot, PublishedReservations, Reservation, CAPACITY_PREFIX};

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
    // Exhausted means the reservations leave nothing: no whole core or no
    // RAM. What a job needs beyond that is the job's own declaration, which
    // the claim checks against these same net numbers.
    let reservations_exhausted =
        !held.live.is_empty() && (net_cpu <= 0 || net_ram.is_some_and(|free| free <= 0.0));
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
    let body = super::super::python_json_dumps(&Value::Object(payload))?;
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
