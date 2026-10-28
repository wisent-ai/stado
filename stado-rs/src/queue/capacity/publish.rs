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
/// How long this process's last capacity publication took to write: the part
/// of the promise the period alone does not cover. `None` until one has been
/// written and measured.
static LAST_PUBLISH: std::sync::Mutex<Option<std::time::Duration>> = std::sync::Mutex::new(None);

/// Declare the period at which this process republishes its capacity. The
/// agent calls this with its own poll period before its first publication.
pub fn declare_publisher_cadence(period: std::time::Duration) {
    let _ = PUBLISHER_CADENCE.set(period);
}

/// The last measured write, refused when another publication panicked while
/// holding it.
fn last_publish() -> Result<Option<std::time::Duration>, StorageError> {
    LAST_PUBLISH.lock().map(|last| *last).map_err(|error| {
        StorageError::Other(format!(
            "the last capacity write time cannot be read: {error}"
        ))
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
/// a Jeden session or a browser task holds it. A reservation subtracts at most
/// what was measured, so a host held beyond its measurement publishes nothing
/// left rather than a negative amount. The measured numbers stay in
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
    // Holds that could not be read are not taken for no holds: the host stops
    // admitting work, says why, and publishes its measured figures with the
    // reservations unknown rather than as free.
    let held = match PublishedReservations::read(store, consumer_id, now).await {
        Ok(held) => Some(held),
        Err(error) => {
            diag.insert(
                "reservations_error".into(),
                Value::String(error.to_string()),
            );
            None
        }
    };
    let measured_cpu = snapshot.available_cpu_cores;
    let reserved = held.as_ref().map(|held| held.reserved);
    // An unmeasured CPU stays unmeasured after netting: null, not zero.
    let net_cpu = measured_cpu.map(|measured| match &reserved {
        Some(reserved) => measured - reserved.cpu_cores.min(measured),
        None => measured,
    });
    let net_ram = snapshot.free_ram_gb.map(|free| match &reserved {
        Some(reserved) => free - reserved.ram_gb.min(free),
        None => free,
    });
    // VRAM the pass did not read stays unread after netting: null, not zero.
    let net_vram = snapshot.free_vram_gb.map(|free| match &reserved {
        Some(reserved) => free - reserved.vram_gb.min(free),
        None => free,
    });
    // Exhausted means the reservations leave nothing: no whole core or no
    // RAM. What a job needs beyond that is the job's own declaration, which
    // the claim checks against these same net numbers.
    let reservations_exhausted = held.as_ref().is_some_and(|held| {
        !held.live.is_empty()
            && (measured_cpu.is_some_and(|measured| held.reserved.cpu_cores >= measured)
                || snapshot
                    .free_ram_gb
                    .is_some_and(|free| held.reserved.ram_gb >= free))
    });
    let mut accepting = snapshot.accepting_jobs;
    if accepting && held.is_none() {
        accepting = false;
        diag.insert(
            "admission_reason".into(),
            Value::from("reservations_unread"),
        );
    } else if accepting && reservations_exhausted {
        accepting = false;
        diag.insert(
            "admission_reason".into(),
            Value::from("reservations_exhausted"),
        );
    }
    if let Some(measured) = measured_cpu {
        diag.insert("measured_available_cpu_cores".into(), Value::from(measured));
    }
    if let Some(free) = snapshot.free_ram_gb {
        diag.insert("measured_free_ram_gb".into(), Value::from(free));
    }
    if let Some(free) = snapshot.free_vram_gb {
        diag.insert("measured_free_vram_gb".into(), Value::from(free));
    }
    let mut payload = Map::new();
    payload.insert("consumer_id".into(), Value::String(consumer_id.to_string()));
    payload.insert("kind".into(), Value::String(kind.to_string()));
    payload.insert("published_at".into(), Value::String(now.to_rfc3339()));
    if let Some(period) = PUBLISHER_CADENCE.get() {
        // Before the first write is measured, the promise is the period alone.
        let promised = match last_publish()? {
            Some(write) => *period + write,
            None => *period,
        };
        let promised = Duration::from_std(promised).map_err(|error| {
            StorageError::Other(format!(
                "the capacity publisher's next promise of {promised:?} is not a time span: {error}"
            ))
        })?;
        payload.insert(
            "next_by".into(),
            Value::String((now + promised).to_rfc3339()),
        );
    }
    payload.insert("accepting_jobs".into(), Value::from(accepting));
    payload.insert(
        "running_jobs".into(),
        Value::from(snapshot.running_jobs as i64),
    );
    payload.insert(
        "running_workloads".into(),
        Value::from(held.as_ref().map(|held| held.live.len())),
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
    // Unread reservations are published as unknown (null), never as none.
    payload.insert("reserved".into(), serde_json::to_value(reserved)?);
    payload.insert(
        "reservations".into(),
        Value::from(held.as_ref().map(|held| {
            held.live
                .iter()
                .map(Reservation::published)
                .collect::<Vec<Value>>()
        })),
    );
    // Names only, never values: the `item#field` references a job may project
    // here, so a coordinator does not pin a job this agent cannot resolve. Read
    // as the config holds them now, so a secret enrolled after this agent
    // started is offered without a restart; a config file that cannot be read
    // stops the publication with its cause.
    payload.insert(
        "secret_fields".into(),
        Value::from(
            crate::config::agent_skarbiec_secret_fields_now().map_err(StorageError::Other)?,
        ),
    );
    payload.insert("diag".into(), Value::Object(diag));
    payload.insert(
        "stado_version".into(),
        Value::String(env!("CARGO_PKG_VERSION").to_string()),
    );
    let body = super::super::python_json_dumps(&Value::Object(payload))?;
    let started = std::time::Instant::now();
    store
        .upload_text(&format!("{CAPACITY_PREFIX}{consumer_id}.json"), &body)
        .await?;
    let mut last = LAST_PUBLISH.lock().map_err(|error| {
        StorageError::Other(format!(
            "the last capacity write time cannot be kept: {error}"
        ))
    })?;
    *last = Some(started.elapsed());
    Ok(())
}
