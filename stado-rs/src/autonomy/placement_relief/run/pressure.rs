//! Which hosts count as pressured this tick.
//!
//! A host that says it is pressured right now stamps this instant; the rest
//! keep whatever the previous report remembered, and anything older than the
//! sticky window is dropped, so the map cannot grow without bound and a host
//! that stopped reporting does not stay pressured for ever.

use std::collections::BTreeMap;

use chrono::{DateTime, Utc as UtcTz};

use super::super::{HostMemory, PRESSURE_STICKY_SECONDS};

/// The hosts that have published memory pressure inside the sticky window:
/// this tick's own pressured hosts stamped now, the previous report's
/// entries kept while they are still inside the window, and everything older
/// dropped.
pub(super) fn pressure_window(
    hosts: &BTreeMap<String, HostMemory>,
    previous: &BTreeMap<String, String>,
    created_at: &str,
    now: DateTime<UtcTz>,
) -> BTreeMap<String, String> {
    let mut seen: BTreeMap<String, String> = previous
        .iter()
        .filter(|(_, last)| {
            DateTime::parse_from_rfc3339(last).is_ok_and(|last| {
                let age = now
                    .signed_duration_since(last.with_timezone(&UtcTz))
                    .num_seconds();
                age >= i64::default() && age < PRESSURE_STICKY_SECONDS
            })
        })
        .map(|(host, last)| (host.clone(), last.clone()))
        .collect();
    for (host, memory) in hosts {
        if memory.pressure_active && !memory.stale {
            seen.insert(host.clone(), created_at.to_string());
        }
    }
    seen
}
