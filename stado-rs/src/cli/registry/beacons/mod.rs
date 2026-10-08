//! Live host state: one `host_health/<slug>.json` object ([`beacon`]), every
//! beacon in the store ([`load`]), and `host beacon list` ([`age`]).
//!
//! The unit states a beacon reports are stated once here because `doctor`
//! and `host beacon list` grade the same signal. Whether a beacon is current is the
//! beacon's own promise ([`beacon::Beacon::next_by`]), never a window here.

pub(in crate::cli::registry) mod age;
pub(in crate::cli::registry) mod beacon;
pub(in crate::cli::registry) mod load;

/// The state a live launchd/systemd unit reports.
pub(in crate::cli::registry) const ACTIVE_STATE: &str = "active";
/// A successful timer-triggered oneshot with an active native trigger.
///
/// This is intentionally distinct from [`ACTIVE_STATE`]: it proves scheduled
/// lifecycle health, not a continuously running process.
const SCHEDULED_STATE: &str = "scheduled";
