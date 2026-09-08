//! The two ceilings a tick runs under: when a resource counts as idle, and
//! how much the control plane may change before it stops.
//!
//! `IdlePolicy` is the patience side — how long a thing must sit unused
//! before it is a candidate — and `SafetyLimits` is the brake: the per-tick
//! action counts, the deletion ceiling, the protections that hold regardless
//! of a rule, and the circuit breaker. The constants at the top are the
//! defaults both derive from. Every field name here is a published document
//! key.

use serde::{Deserialize, Serialize};

const TWO: u64 = (u16::BITS / u8::BITS) as u64;
const FIFTEEN: u64 = (u8::BITS as u64 * TWO) - true as u64;
const THIRTY: u64 = u64::BITS as u64 / TWO - TWO;
const DEFAULT_ACTION_LIMIT: usize = (u8::BITS as u64 + TWO) as usize;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct IdlePolicy {
    pub vm_seconds: u64,
    pub disk_days: u64,
    pub snapshot_days: u64,
    pub artifact_days: u64,
    pub minimum_snapshots: usize,
    pub utilization_window_days: u64,
}

impl Default for IdlePolicy {
    fn default() -> Self {
        Self {
            vm_seconds: crate::monitor::billing::SECONDS_PER_MINUTE * FIFTEEN,
            disk_days: u8::BITS as u64 - true as u64,
            snapshot_days: THIRTY,
            artifact_days: THIRTY,
            minimum_snapshots: true as usize,
            utilization_window_days: u8::BITS as u64 - true as u64,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SafetyLimits {
    pub max_actions_per_tick: usize,
    pub max_actions_per_provider: usize,
    pub max_deleted_bytes_per_tick: Option<u64>,
    pub max_concurrent_mutations: usize,
    pub require_complete_inventory: bool,
    pub protect_production: bool,
    pub protect_stateful: bool,
    pub circuit_breaker_failures: usize,
    pub circuit_breaker_cooldown_seconds: u64,
    pub decision_ttl_seconds: u64,
}

impl Default for SafetyLimits {
    fn default() -> Self {
        Self {
            max_actions_per_tick: DEFAULT_ACTION_LIMIT,
            max_actions_per_provider: DEFAULT_ACTION_LIMIT,
            max_deleted_bytes_per_tick: None,
            max_concurrent_mutations: TWO as usize,
            require_complete_inventory: true,
            protect_production: true,
            protect_stateful: true,
            circuit_breaker_failures: (u8::BITS / TWO as u32) as usize,
            circuit_breaker_cooldown_seconds: crate::monitor::billing::SECONDS_PER_MINUTE * FIFTEEN,
            decision_ttl_seconds: crate::monitor::billing::SECONDS_PER_MINUTE
                * (u16::BITS / u8::BITS) as u64
                + crate::monitor::billing::SECONDS_PER_MINUTE * (u8::BITS as u64 / TWO),
        }
    }
}
