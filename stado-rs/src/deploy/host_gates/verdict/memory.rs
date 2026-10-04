//! The memory half of one host's claiming verdict: readings, never a
//! blocker.
//!
//! No host declares memory handling, so nothing here withholds a host from
//! work. The verdict still carries what the host has left, because a host that
//! has stopped answering is most often one that has run out of memory, and
//! the operator reading `stado host gates` needs that number beside the
//! disk's.

use serde_json::{json, Value};

use crate::deploy::host_disk::DiskReading;
use crate::deploy::host_gates::gates::HostGates;
use crate::providers::local::host_memory::gigabytes;

/// Where the memory half of the verdict was read.
///
/// The host's own live publication when it is talking, and this command's own
/// measurement of the host when it is not: the host whose agent has no memory
/// left to publish with is exactly the one this reading matters most for.
pub const MEMORY_SOURCE_PUBLICATION: &str = "capacity_publication";
pub const MEMORY_SOURCE_MEASUREMENT: &str = "host_memory_measurement";

/// What this host has left of its memory, as the verdict carries it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MemoryGate {
    pub available_gb: Option<f64>,
    pub total_gb: Option<f64>,
    pub swap_used_pct: Option<i64>,
    /// Which of the two sources this reading came from, or `None` when
    /// neither answered.
    pub source: Option<&'static str>,
}

impl MemoryGate {
    /// The `memory` object of the `--json` report, the twin of `disk`.
    pub fn to_value(&self) -> Value {
        json!({
            "available_gb": self.available_gb,
            "total_gb": self.total_gb,
            "swap_used_pct": self.swap_used_pct,
            "source": self.source,
        })
    }

    /// The operator's line, or `None` when nothing about this host's memory
    /// was read. Silence is reported as silence, never as health.
    pub fn line(&self) -> Option<String> {
        if self.available_gb.is_none() && self.swap_used_pct.is_none() {
            return None;
        }
        let mut clauses = vec![match (self.available_gb, self.total_gb) {
            (Some(free), Some(total)) => format!("available {free} of {total} GiB"),
            (Some(free), None) => format!("available {free} GiB"),
            (None, _) => "availability not observed".to_string(),
        }];
        if let Some(used) = self.swap_used_pct {
            clauses.push(format!("swap {used}% used"));
        }
        Some(clauses.join(", "))
    }
}

/// Read the memory half out of a live capacity publication. A stale
/// publication is not a memory reading - the agent has stopped talking, and
/// `capacity_publication_stale` already says that.
pub fn apply(gates: &mut HostGates, payload: Option<&Value>, publication_current: bool) {
    if !publication_current {
        return;
    }
    let diag = payload.and_then(|value| value.get("diag"));
    gates.memory = MemoryGate {
        available_gb: number(diag, "memory_available_gb"),
        total_gb: number(diag, "memory_total_gb"),
        swap_used_pct: diag
            .and_then(|diag| diag.get("memory_swap_used_pct"))
            .and_then(Value::as_i64),
        source: Some(MEMORY_SOURCE_PUBLICATION),
    };
}

/// Read the memory half out of this command's own measurement of the host,
/// for a host whose publication is absent or stale.
pub fn apply_measured(gates: &mut HostGates, reading: &DiskReading) {
    let memory = &reading.memory;
    if memory.available_bytes.is_none() && memory.swap_used_pct().is_none() {
        return;
    }
    gates.memory = MemoryGate {
        available_gb: gigabytes(memory.available_bytes),
        total_gb: gigabytes(memory.total_bytes),
        swap_used_pct: memory.swap_used_pct(),
        source: Some(MEMORY_SOURCE_MEASUREMENT),
    };
}

fn number(diag: Option<&Value>, field: &str) -> Option<f64> {
    diag.and_then(|diag| diag.get(field))
        .and_then(Value::as_f64)
}
