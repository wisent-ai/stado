//! The one path a placed workload takes its hold through: read the host's
//! publication, check the declared reservation fits what is net free, take
//! it, and keep it heartbeated until the caller lets go.

use std::num::NonZeroU64;

use chrono::Utc;

use crate::cli::registry::read_registry;
use crate::cli::reporting::NOT_REPORTED;
use crate::cli::workload::{WorkloadKind, WorkloadReservation};
use crate::cli::CmdError;
use crate::fleet_needs::{
    record_unmet, this_requester, Candidate, Requirement, UnmetPlacement, UnmetReason,
};
use crate::queue::capacity::{
    consumer_id_for_target, publication_of, publication_period, read_publications, reservations,
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
        CmdError::refused(self.sentence)
    }
}

/// What the host publishes now, netted: cores, RAM, and how many holds. A
/// publication from before workloads or admission reasons were published
/// carries neither, and they stay unstated rather than read as none.
struct NetRoom {
    cores: i64,
    ram_gb: Option<f64>,
    held: Option<u64>,
    accepting: bool,
    reason: Option<String>,
}

/// The room `publication` states, or the refusal naming the field it does
/// not state: without its free cores or whether it accepts placements, no
/// one can judge whether the hold fits, and reading either as zero or no
/// would refuse for a reason the host never gave.
fn net_room(
    publication: &Publication,
    target: &ComputeTarget,
) -> Result<NetRoom, ReservationRefusal> {
    let payload = &publication.payload;
    let unstated = |field: &str| ReservationRefusal {
        target: target.name.clone(),
        sentence: format!(
            "{} publishes no {field} in its capacity row, so whether the hold fits cannot be \
             judged; update its agent or pick another host with --target",
            target.name
        ),
        reason: UnmetReason::NoEligibleTarget,
    };
    let Some(cores) = payload["available_cpu_cores"].as_i64() else {
        return Err(unstated("available_cpu_cores"));
    };
    let Some(accepting) = payload["accepting_jobs"].as_bool() else {
        return Err(unstated("accepting_jobs"));
    };
    Ok(NetRoom {
        cores,
        ram_gb: payload["free_ram_gb"].as_f64(),
        held: payload["running_workloads"].as_u64(),
        accepting,
        reason: payload["diag"]["admission_reason"]
            .as_str()
            .map(str::to_string),
    })
}

/// Take `kind`'s declared reservation on `target`, or say exactly why not.
///
/// A host whose publication is past its promise is taken on trust: its agent
/// is silent, and refusing every placement on a quiet host would make the
/// primitive a liability the day a store hiccups. The hold is still written,
/// so the next publication subtracts it. A host that has never published
/// states no period, so a hold on it could promise nothing and is refused.
pub async fn reserve_for_workload(
    kind: &WorkloadKind,
    target: &ComputeTarget,
    holder: String,
) -> Result<Result<reservations::HeldReservation, ReservationRefusal>, CmdError> {
    let wanted = kind.reservation()?;
    let registry = read_registry().await?;
    let store = crate::queue::submit::default_store("")
        .await
        .map_err(CmdError::from)?;
    let publications = read_publications(&store).await.map_err(CmdError::from)?;
    let consumer_id =
        consumer_id_for_target(&registry, target, &publications).map_err(CmdError::from)?;
    let publication = publication_of(&registry, target, &publications)
        .map_err(CmdError::from)?
        .map(|(_, publication)| publication);
    if let Some(publication) = publication.filter(|row| !row.stale(Utc::now())) {
        let refused = match net_room(publication, target) {
            Ok(room) => refusal(kind, &wanted, target, &room),
            Err(unstated) => Some(unstated),
        };
        if let Some(refusal) = refused {
            record_refusal(&store, kind, &wanted, &refusal).await;
            return Ok(Err(refusal));
        }
    }
    let Some(period) = publication.and_then(|row| publication_period(&row.payload)) else {
        let refusal = ReservationRefusal {
            target: target.name.clone(),
            sentence: format!(
                "{} publishes no capacity row that states its period ({consumer_id}), so \
                 nothing would read a reservation there and the hold could promise no time; \
                 start its agent or pick another host with --target",
                target.name
            ),
            reason: UnmetReason::NoEligibleTarget,
        };
        record_refusal(&store, kind, &wanted, &refusal).await;
        return Ok(Err(refusal));
    };
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
            promise: period,
        },
    )
    .await
    .map_err(|error| {
        let message = format!("cannot reserve {} on {}: {error}", kind.kind, target.name);
        let mut failed = CmdError::from(error);
        failed.message = Some(message);
        failed
    })?;
    Ok(Ok(reservations::hold(lease, store, period)))
}

fn refusal(
    kind: &WorkloadKind,
    wanted: &WorkloadReservation,
    target: &ComputeTarget,
    room: &NetRoom,
) -> Option<ReservationRefusal> {
    let fits =
        room.cores >= wanted.cpu_cores && room.ram_gb.is_none_or(|free| free >= wanted.ram_gb);
    if !fits {
        let free_ram = match room.ram_gb {
            Some(free) => free.to_string(),
            None => NOT_REPORTED.to_string(),
        };
        let held = match room.held {
            Some(held) => held.to_string(),
            None => NOT_REPORTED.to_string(),
        };
        return Some(ReservationRefusal {
            target: target.name.clone(),
            sentence: format!(
                "{} has no room for {}: needs {} cores and {} GiB, host publishes {} cores and {free_ram} GiB net of {held} reservation(s); pick another host with --target or wait for a reservation to end",
                target.name,
                kind.kind,
                wanted.cpu_cores,
                wanted.ram_gb,
                room.cores,
            ),
            reason: match room.held.and_then(NonZeroU64::new) {
                Some(_) => UnmetReason::ReservationsExhausted,
                None => UnmetReason::CapacityExhausted,
            },
        });
    }
    // A host that is not accepting but gives no reason still takes a
    // workload hold: its job admission is what it closed, not its room.
    let reason = match room.reason.as_deref() {
        Some(reason) if !room.accepting && !reason.is_empty() => reason,
        _ => return None,
    };
    let reason_kind = match reason {
        "disk_pressure_active"
        | "disk_unreadable"
        | "disk_gate_refused"
        | "cleanup_in_progress"
        | "space_reclaim_lock_held" => UnmetReason::DiskPressure,
        "reservations_exhausted" => UnmetReason::ReservationsExhausted,
        _ => UnmetReason::CapacityExhausted,
    };
    Some(ReservationRefusal {
        target: target.name.clone(),
        sentence: format!(
            "{} is not accepting placements ({reason}); pick another host with --target or wait for it to clear",
            target.name
        ),
        reason: reason_kind,
    })
}

async fn record_refusal(
    store: &JobStorage,
    kind: &WorkloadKind,
    wanted: &WorkloadReservation,
    refusal: &ReservationRefusal,
) {
    let requester = match this_requester() {
        Ok(requester) => requester,
        Err(error) => {
            eprintln!("the refusal could not be recorded for `stado fleet needs`: {error}");
            return;
        }
    };
    let record = UnmetPlacement::new(
        &kind.kind,
        &kind.product,
        requester,
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
        eprintln!("the refusal could not be recorded for `stado fleet needs`: {error}");
    }
}
