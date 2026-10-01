//! The two ceilings a tick runs under: when a resource counts as idle, and
//! how much the control plane may change before it stops.
//!
//! `IdlePolicy` is the patience side — how long a thing must sit unused
//! before it is a candidate — and `SafetyLimits` is the brake: the per-tick
//! action counts, the deletion ceiling, the protections that hold regardless
//! of a rule, and the circuit breaker. Neither has a default: every value is
//! one the operator wrote into the policy document. Every field name here is
//! a published document key.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IdlePolicy {
    pub vm_seconds: u64,
    pub disk_days: u64,
    pub snapshot_days: u64,
    pub artifact_days: u64,
    pub minimum_snapshots: usize,
    pub utilization_window_days: u64,
    /// A resource whose every observed peak, as a ratio of its capacity,
    /// sits below this is a rightsizing candidate.
    pub underutilized_below: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SafetyLimits {
    pub max_actions_per_tick: usize,
    pub max_actions_per_provider: usize,
    #[serde(default)]
    pub max_deleted_bytes_per_tick: Option<u64>,
    pub max_concurrent_mutations: usize,
    pub require_complete_inventory: bool,
    pub protect_production: bool,
    pub protect_stateful: bool,
    pub circuit_breaker_failures: usize,
    pub circuit_breaker_cooldown_seconds: u64,
    pub decision_ttl_seconds: u64,
}
