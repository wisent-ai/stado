//! Live host state: one `host_health/<slug>.json` object ([`beacon`]), every
//! beacon in the store ([`load`]), and `registry beacon-age` ([`age`]).
//!
//! The unit states a beacon reports, and the window past which one is a
//! divergence rather than jitter, are stated once here because `doctor` and
//! `beacon-age` grade the same signal.

pub(in crate::cli::registry) mod age;
pub(in crate::cli::registry) mod beacon;
pub(in crate::cli::registry) mod load;

use crate::queue::capacity;

/// The state a live launchd/systemd unit reports
/// (`deploy/beacon/host_health_beacon_macos.sh`, `deploy/beacon/host_health_beacon.sh`).
pub(in crate::cli::registry) const ACTIVE_STATE: &str = "active";
/// A successful timer-triggered oneshot with an active native trigger.
///
/// This is intentionally distinct from [`ACTIVE_STATE`]: it proves scheduled
/// lifecycle health, not a continuously running process.
const SCHEDULED_STATE: &str = "scheduled";

/// A beacon older than the fleet's liveness window is a divergence, not
/// jitter: the beacon republishes on the same cadence as the capacity
/// broadcast (`constants::CAPACITY_HEARTBEAT_INTERVAL_S` seconds — the
/// LaunchAgent `StartInterval` rendered by
/// `deploy/install/install_macos_coordinator.sh`, and the systemd unit in
/// `deploy/units/host-health-beacon.service`), so
/// [`capacity::CAPACITY_STALE_SECONDS`] is the same missed-publications
/// window `queue::capacity` already applies to the other liveness signal.
/// One window, both signals.
pub(in crate::cli::registry) fn stale_after_seconds() -> i64 {
    capacity::CAPACITY_STALE_SECONDS as i64
}
