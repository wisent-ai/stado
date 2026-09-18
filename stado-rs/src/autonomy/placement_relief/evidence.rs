//! What each host says about its own memory, read from the capacity
//! publication its agent writes and nowhere else.
//!
//! The fields are the ones the host's memory policy publishes beside its
//! admission verdict ([`crate::providers::local::host_memory::report`]):
//! `memory_pressure_active`, `memory_available_gb`, `memory_total_gb`,
//! `memory_swap_used_pct`, `memory_swap_pressure_only`. Read verbatim, so a
//! relocation can only ever be argued from numbers the host itself put in
//! writing and `stado host gates` shows.

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::queue::capacity::{consumer_names_target, Publication};
use crate::targets::Registry;

/// One host's memory, as it published it.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct HostMemory {
    /// Seconds since the publication, or `None` for an undateable row.
    pub age_seconds: Option<i64>,
    /// Past the fleet's capacity staleness horizon.
    pub stale: bool,
    pub pressure_active: bool,
    pub swap_pressure_only: bool,
    pub available_gb: Option<f64>,
    pub total_gb: Option<f64>,
    pub swap_used_pct: Option<f64>,
}

impl HostMemory {
    fn from_publication(publication: &Publication, now: DateTime<Utc>) -> Self {
        let diag = publication.payload.get("diag");
        let flag = |key: &str| {
            diag.and_then(|diag| diag.get(key))
                .and_then(Value::as_bool)
                .unwrap_or(false)
        };
        let number = |key: &str| diag.and_then(|diag| diag.get(key)).and_then(Value::as_f64);
        Self {
            age_seconds: publication.age_seconds(now),
            stale: publication.stale(now),
            pressure_active: flag("memory_pressure_active"),
            swap_pressure_only: flag("memory_swap_pressure_only"),
            available_gb: number("memory_available_gb"),
            total_gb: number("memory_total_gb"),
            swap_used_pct: number("memory_swap_used_pct"),
        }
    }

    /// One line an operator reads beside a decision about this host.
    pub fn describe(&self) -> String {
        let gb = |value: Option<f64>| {
            value.map_or_else(|| "?".to_string(), |value| format!("{value:.1}"))
        };
        let age = self
            .age_seconds
            .map_or_else(|| "unknown age".to_string(), |age| format!("{age}s ago"));
        format!(
            "{} GiB available of {} GiB, swap {}%, pressure {}, published {age}",
            gb(self.available_gb),
            gb(self.total_gb),
            self.swap_used_pct
                .map_or_else(|| "?".to_string(), |pct| format!("{pct:.0}")),
            if self.pressure_active {
                "active"
            } else {
                "clear"
            },
        )
    }
}

/// Every local target's memory, keyed by registry name. A host with no
/// publication has no entry: absence is a fact the planner reports as such,
/// never a healthy host.
pub fn host_memory(
    registry: &Registry,
    publications: &BTreeMap<String, Publication>,
    now: DateTime<Utc>,
) -> BTreeMap<String, HostMemory> {
    registry
        .targets
        .iter()
        .filter(|target| target.kind == "local")
        .filter_map(|target| {
            publications
                .iter()
                .find(|(consumer, _)| consumer_names_target(registry, target, consumer))
                .map(|(_, publication)| {
                    (
                        target.name.clone(),
                        HostMemory::from_publication(publication, now),
                    )
                })
        })
        .collect()
}
