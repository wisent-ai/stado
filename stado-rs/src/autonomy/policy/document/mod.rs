//! The policy document itself, and the two things it is asked to do.
//!
//! `AutonomyPolicy` is the versioned document an operator writes: the mode,
//! the pause switch, the knob groups and the ordered resource rules. The
//! components are the two questions asked of it — `validate` refuses a
//! document that cannot be honoured, and `authorize` decides a single
//! proposed mutation and returns the verdict with its reason. `stateful`
//! answers the one question authorization cannot answer from the document
//! alone: whether the resource in front of it holds data. Every field name
//! here is a published document key.

mod authorize;
mod stateful;
mod validate;

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::{
    AutonomyMode, BudgetPolicy, FreshnessPolicy, IdlePolicy, PlacementPolicy, ResourceRule,
    SafetyLimits,
};

pub use authorize::AuthorizationDecision;

/// No document means no autonomy: the idle, freshness and safety values have
/// no defaults, so a tick runs only once an operator has written them.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AutonomyPolicy {
    pub policy_version: String,
    #[serde(default)]
    pub mode: AutonomyMode,
    #[serde(default)]
    pub emergency_paused: bool,
    #[serde(default)]
    pub budgets: BudgetPolicy,
    #[serde(default)]
    pub placement: PlacementPolicy,
    pub idle: IdlePolicy,
    pub freshness: FreshnessPolicy,
    pub limits: SafetyLimits,
    #[serde(default)]
    pub local_hourly_cost_usd: Option<f64>,
    #[serde(default)]
    pub rules: Vec<ResourceRule>,
    #[serde(default)]
    pub metadata: BTreeMap<String, String>,
}
