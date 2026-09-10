//! The memory half of one host's claiming verdict.
//!
//! The disk half of this verdict is complete: free bytes, the watermark it is
//! measured against, the target, the policy mode, whether the janitor is
//! keeping up, and a blocker when the pressure is unresolved. Memory had two
//! totals and a raw flag inside `published_diagnostics`.
//!
//! On 2026-09-10 `skarbiec` could not publish `linux-amd64`: the only Linux
//! builder was refusing placement, and `stado host gates
//! ubuntu-server-rtx-pro-6000` said `accepting_jobs: false` with 62.8 GiB of
//! 123.0 GiB free RAM, `blockers: [host_diagnostic_incomplete]`, and nothing
//! about memory at all. The reason was published; no surface read it.

use chrono::{DateTime, Utc};
use serde_json::{json, Value};

use super::payload::diag_flag;
use crate::deploy::host_gates::gates::HostGates;

/// The admission reason a host publishes while its memory declaration refuses
/// placement. The same word the publisher writes, read back here.
pub const MEMORY_PRESSURE_ACTIVE: &str =
    crate::providers::local::host_memory::MEMORY_PRESSURE_ACTIVE;

/// What this host published about its own memory, as the verdict carries it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MemoryGate {
    /// The host's own declaration is refusing new work right now.
    pub pressure_active: bool,
    /// Whether the declaration refuses placement at all, or `None` where the
    /// host published no answer. A host that declares `refuse_placement:
    /// false` reports its pressure and keeps taking work, and an operator
    /// reading "no memory blocker" has to be able to see which of the two it
    /// is looking at.
    pub refuse_placement: Option<bool>,
    pub available_gb: Option<f64>,
    pub total_gb: Option<f64>,
    /// The low watermark the reading was measured against.
    pub low_watermark_gb: Option<f64>,
    pub swap_used_pct: Option<i64>,
    pub swap_high_watermark_pct: Option<i64>,
    /// The declared mode of the pass that maintains this host's memory.
    pub policy_mode: Option<String>,
    /// Seconds since the memory pass last completed, or `None` when it has
    /// never recorded one - the same distinction the disk janitor's age keeps.
    pub pass_success_age_seconds: Option<i64>,
    pub pass_outcome: Option<String>,
}

impl MemoryGate {
    /// The `memory` object of the `--json` report, the twin of `disk`.
    pub fn to_value(&self) -> Value {
        json!({
            "pressure_active": self.pressure_active,
            "refuse_placement": self.refuse_placement,
            "available_gb": self.available_gb,
            "total_gb": self.total_gb,
            "low_watermark_gb": self.low_watermark_gb,
            "swap_used_pct": self.swap_used_pct,
            "swap_high_watermark_pct": self.swap_high_watermark_pct,
            "policy_mode": self.policy_mode,
            "pass_success_age_seconds": self.pass_success_age_seconds,
            "pass_outcome": self.pass_outcome,
        })
    }

    /// The operator's line, or `None` when this host published nothing about
    /// its memory. Silence is reported as silence, never as health.
    pub fn line(&self) -> Option<String> {
        if self.refuse_placement.is_none() && !self.pressure_active && self.available_gb.is_none() {
            return None;
        }
        let mut clauses = vec![match (self.available_gb, self.low_watermark_gb) {
            (Some(free), Some(low)) => {
                format!("available {free} GiB against a {low} GiB watermark")
            }
            (Some(free), None) => format!("available {free} GiB, no watermark published"),
            (None, Some(low)) => format!("availability not observed, watermark {low} GiB"),
            (None, None) => "no memory measurement published".to_string(),
        }];
        if let (Some(used), Some(high)) = (self.swap_used_pct, self.swap_high_watermark_pct) {
            clauses.push(format!("swap {used}% against {high}%"));
        }
        clauses.push(if self.pressure_active {
            format!("refusing placement ({MEMORY_PRESSURE_ACTIVE})")
        } else if self.refuse_placement == Some(false) {
            "reporting only; this host does not refuse placement".to_string()
        } else {
            "taking work".to_string()
        });
        Some(clauses.join(", "))
    }
}

/// Read the memory half out of a live capacity publication and apply it.
///
/// The refusal becomes a blocker for the same reason `disk_pressure_active`
/// is one: the host has declared that it will not run the work, so a verdict
/// that called it claimable would send every release build to a machine that
/// refuses them. A stale publication is not a memory diagnosis - the agent has
/// stopped talking, and `capacity_publication_stale` already says that.
pub fn apply(
    gates: &mut HostGates,
    payload: Option<&Value>,
    publication_current: bool,
    now: DateTime<Utc>,
) {
    if !publication_current {
        return;
    }
    let diag = payload.and_then(|value| value.get("diag"));
    let report = diag.and_then(|diag| diag.get("memory_reclaim"));
    gates.memory = MemoryGate {
        pressure_active: diag_flag(payload, MEMORY_PRESSURE_ACTIVE) == Some(true),
        refuse_placement: diag_flag(payload, "memory_refuse_placement"),
        available_gb: number(diag, "memory_available_gb"),
        total_gb: number(diag, "memory_total_gb"),
        low_watermark_gb: number(diag, "memory_low_watermark_gb"),
        swap_used_pct: integer(diag, "memory_swap_used_pct"),
        swap_high_watermark_pct: integer(diag, "memory_swap_high_watermark_pct"),
        policy_mode: text(report, "mode"),
        pass_success_age_seconds: text(report, "last_success_at")
            .as_deref()
            .and_then(|stamp| DateTime::parse_from_rfc3339(&stamp.replace('Z', "+00:00")).ok())
            .map(|stamp| (now - stamp.with_timezone(&Utc)).num_seconds()),
        pass_outcome: text(report, "outcome"),
    };
    if gates.memory.pressure_active {
        gates.blockers.push(MEMORY_PRESSURE_ACTIVE.to_string());
        gates.claiming = false;
    }
}

fn number(diag: Option<&Value>, field: &str) -> Option<f64> {
    diag.and_then(|diag| diag.get(field))
        .and_then(Value::as_f64)
}

fn integer(diag: Option<&Value>, field: &str) -> Option<i64> {
    diag.and_then(|diag| diag.get(field))
        .and_then(Value::as_i64)
}

fn text(report: Option<&Value>, field: &str) -> Option<String> {
    report
        .and_then(|report| report.get(field))
        .and_then(Value::as_str)
        .map(str::to_string)
}
