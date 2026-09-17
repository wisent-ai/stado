//! The one path a placed workload takes its hold through: read the host's
//! publication, check the declared reservation fits what is net free, take
//! it, and keep it heartbeated until the caller lets go.

use chrono::Utc;
use serde_json::Value;

use crate::cli::registry::read_registry;
use crate::cli::workload::{WorkloadKind, WorkloadReservation};
use crate::cli::CmdError;
use crate::fleet_needs::{
    record_unmet, this_requester, Candidate, Requirement, UnmetPlacement, UnmetReason,
};
use crate::queue::capacity::{
    consumer_id_for_target, consumer_names_target, read_publications, reservations,
    Publication,
};
use crate::queue::JobStorage;
use crate::targets::ComputeTarget;

/// Why a host could not take the hold, with the sentence the operator sees.
#[derive(Debug, Clone, PartialEq)]
pub struct ReservationRefusal {
    pub target: String,
    pub sentence: String,
    pub reason: UnmetReason,
}

impl ReservationRefusal {
    pub fn into_error(self) -> CmdError {
        CmdError::click(self.sentence)
    }
}

/// What the host publishes now, netted: cores, RAM, and how many holds.
struct NetRoom {
    cores: i64,
    ram_gb: Option<f64>,
    held: usize,
    accepting: bool,
    reason: String,
    stale: bool,
}

fn net_room(publication: &Publication) -> NetRoom {
    let payload = &publication.payload;
    NetRoom {
        cores: payload
            .get("available_cpu_cores")
            .and_then(Value::as_i64)
            .unwrap_or(0),
        ram_gb: payload.get("free_ram_gb").and_then(Value::as_f64),
        held: payload
            .get("running_workloads")
            .and_then(Value::as_i64)
            .unwrap_or(0) as usize,
        accepting: payload
            .get("accepting_jobs")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        reason: payload
            .get("diag")
            .and_then(|diag| diag.get("admission_reason"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        stale: publication.stale(Utc::now()),
    }
}

/// Take `kind`'s declared reservation on `target`, or say exactly why not.
///
/// A host with no fresh publication is taken on trust: its agent is silent,
/// and refusing every placement on a quiet host would make the primitive a
/// liability the day a store hiccups. The hold is still written, so the next
/// publication subtracts it.
pub async fn reserve_for_workload(
    kind: &WorkloadKind,
    target: &ComputeTarget,
    holder: String,
) -> Result<Result<reservations::HeldReservation, ReservationRefusal>, CmdError> {
    let wanted = kind.reservation()?;
    let registry = read_registry().await?;
    let store = crate::queue::submit::default_store("")
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    let publications = read_publications(&store)
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    let consumer_id = consumer_id_for_target(&registry, target, &publications);
    let published = publications
        .iter()
        .find(|(consumer, _)| consumer_names_target(&registry, target, consumer))
        .map(|(_, publication)| net_room(publication));
    if let Some(room) = published.filter(|room| !room.stale) {
        if let Some(refusal) = refusal(kind, &wanted, target, &room) {
            record_refusal(&store, kind, &wanted, &refusal).await;
            return Ok(Err(refusal));
        }
    }
    let lease = reservations::acquire(
        &store,
        reservations::ReservationRequest {
            consumer_id,
            target: target.name.clone(),
            kind: kind.kind.clone(),
            product: kind.product.clone(),
            holder,
            cpu_cores: wanted.cpu_cores,
            ram_gb: wanted.ram_gb,
            vram_gb: wanted.vram_gb,
            ttl_seconds: reservations::DEFAULT_TTL_SECONDS,
        },
    )
    .await
    .map_err(|error| CmdError::click(format!("cannot reserve {} on {}: {error}", kind.kind, target.name)))?;
    Ok(Ok(reservations::hold(
        lease,
        store,
        reservations::HEARTBEAT_INTERVAL,
    )))
}

fn refusal(
    kind: &WorkloadKind,
    wanted: &WorkloadReservation,
    target: &ComputeTarget,
    room: &NetRoom,
) -> Option<ReservationRefusal> {
    let fits = room.cores >= wanted.cpu_cores
        && room.ram_gb.is_none_or(|free| free >= wanted.ram_gb);
    if fits && (room.accepting || room.reason.is_empty()) {
        return None;
    }
    if !fits {
        return Some(ReservationRefusal {
            target: target.name.clone(),
            sentence: format!(
                "{} has no room for {}: needs {} cores and {} GiB, host publishes {} cores and {} GiB net of {} reservation(s); pick another host with --target or wait for a reservation to end",
                target.name,
                kind.kind,
                wanted.cpu_cores,
                wanted.ram_gb,
                room.cores,
                room.ram_gb.map(|free| format!("{free:.1}")).unwrap_or_else(|| "?".to_string()),
                room.held
            ),
            reason: if room.held > 0 {
                UnmetReason::ReservationsExhausted
            } else {
                UnmetReason::CapacityExhausted
            },
        });
    }
    let reason = match room.reason.as_str() {
        crate::providers::local::host_memory::MEMORY_PRESSURE_ACTIVE => UnmetReason::MemoryPressure,
        "disk_gate_refused" | "cleanup_in_progress" | "disk_cleanup_lock_held" => {
            UnmetReason::DiskPressure
        }
        "reservations_exhausted" => UnmetReason::ReservationsExhausted,
        _ => UnmetReason::CapacityExhausted,
    };
    Some(ReservationRefusal {
        target: target.name.clone(),
        sentence: format!(
            "{} is not accepting placements ({}); pick another host with --target or wait for it to clear",
            target.name, room.reason
        ),
        reason,
    })
}

async fn record_refusal(
    store: &JobStorage,
    kind: &WorkloadKind,
    wanted: &WorkloadReservation,
    refusal: &ReservationRefusal,
) {
    let record = UnmetPlacement::new(
        &kind.kind,
        &kind.product,
        this_requester(),
        Requirement {
            platform: None,
            gpu_type: None,
            vram_gb: wanted.vram_gb,
            ram_gb: wanted.ram_gb,
            cpu_cores: wanted.cpu_cores,
            exclusive: false,
            pinned_host: Some(refusal.target.clone()),
        },
        refusal.reason,
        vec![Candidate {
            target: refusal.target.clone(),
            refusal: refusal.sentence.clone(),
        }],
    );
    if let Err(error) = record_unmet(store, &record).await {
        eprintln!(
            "the refusal could not be recorded for `stado fleet needs`: {error}"
        );
    }
}
