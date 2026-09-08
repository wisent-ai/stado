//! The per-tick budget the caller hands the dispatcher.

use std::collections::{BTreeMap, HashMap};

use crate::models::Job;

/// Inputs shared by the caller's per-tick budgets; `available` and
/// `accel_dispatched` are mutated in place so the caller's books stay
/// consistent with cloud reality (Python passes dicts by reference).
pub struct AgentDispatchInputs<'a> {
    pub queued: Vec<Job>,
    pub yield_targets: HashMap<String, String>,
    pub available: &'a mut BTreeMap<String, i64>,
    pub accel_dispatched: &'a mut BTreeMap<String, i64>,
    pub per_accel_share: i64,
    pub per_tick_cap: i64,
    pub scheduled_so_far: i64,
}
