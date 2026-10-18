//! The second copy of a placement refusal: when no declared target can take
//! a workload kind, the refusal is also recorded for `stado fleet needs`, with
//! the room the kind declares and the platform it needs.

use super::catalog::WorkloadKind;
use crate::fleet_needs::{
    record_unmet, this_requester, Candidate, Requirement, UnmetPlacement, UnmetReason,
};

/// The platform a kind needs, when its declaration pins one. Only
/// `gui-automation` does today; every other kind is refused for a missing
/// Weles or mobile declaration, which is a registry gap and not a machine
/// the fleet lacks.
fn required_platform(declaration: &WorkloadKind) -> Option<&'static str> {
    match declaration.kind.as_str() {
        "gui-automation" => Some("darwin-arm64"),
        _ => None,
    }
}

/// Record that no target of `registry` declares `declaration`, naming each
/// target's refusal. A record that cannot be written is said on stderr, as a
/// write that fails is: the refusal itself has already been returned.
pub(super) async fn record_no_eligible_target(
    declaration: &WorkloadKind,
    allowance: Option<&str>,
    registry: &crate::targets::Registry,
) {
    // The catalog refuses a kind without a reservation when it loads, so a
    // kind that reaches here declares one; one that does not is said, not
    // recorded as asking for no room.
    let reservation = match declaration.reservation() {
        Ok(reservation) => reservation,
        Err(error) => {
            eprintln!("the refusal could not be recorded for `stado fleet needs`: {error}");
            return;
        }
    };
    let requester = match this_requester() {
        Ok(requester) => requester,
        Err(error) => {
            eprintln!("the refusal could not be recorded for `stado fleet needs`: {error}");
            return;
        }
    };
    let action = match allowance {
        Some(action) => format!(" with action {action}"),
        None => String::new(),
    };
    let record = UnmetPlacement::new(
        &declaration.kind,
        &declaration.product,
        requester,
        Requirement {
            platform: required_platform(declaration).map(str::to_string),
            gpu_type: None,
            vram_gb: reservation.vram_gb,
            ram_gb: reservation.ram_gb,
            cpu_cores: reservation.cpu_cores,
            exclusive: false,
            pinned_host: None,
        },
        UnmetReason::NoEligibleTarget,
        registry
            .targets
            .iter()
            .map(|target| Candidate {
                target: target.name.clone(),
                refusal: format!("{} declares no {}{action}", target.name, declaration.kind),
            })
            .collect(),
    );
    // The record is the refusal's second copy, for `stado fleet needs`; a
    // store that does not open is said like a write that fails.
    let store = match crate::queue::submit::default_store("").await {
        Ok(store) => store,
        Err(error) => {
            eprintln!(
                "the refusal could not be recorded for `stado fleet needs`: the store did not open: {error}"
            );
            return;
        }
    };
    if let Err(error) = record_unmet(&store, &record).await {
        eprintln!("the refusal could not be recorded for `stado fleet needs`: {error}");
    }
}
