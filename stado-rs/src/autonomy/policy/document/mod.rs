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

use crate::autonomy::model::SCHEMA_VERSION;

use super::{
    AutonomyMode, BudgetPolicy, FreshnessPolicy, IdlePolicy, PlacementPolicy, ResourceRule,
    SafetyLimits,
};

pub use authorize::AuthorizationDecision;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AutonomyPolicy {
    pub schema_version: u16,
    pub policy_version: String,
    pub mode: AutonomyMode,
    pub emergency_paused: bool,
    pub budgets: BudgetPolicy,
    pub placement: PlacementPolicy,
    pub idle: IdlePolicy,
    pub freshness: FreshnessPolicy,
    pub limits: SafetyLimits,
    pub local_hourly_cost_usd: Option<f64>,
    pub rules: Vec<ResourceRule>,
    pub metadata: BTreeMap<String, String>,
}

impl Default for AutonomyPolicy {
    fn default() -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            policy_version: "default-report-only".to_string(),
            mode: AutonomyMode::Report,
            emergency_paused: false,
            budgets: BudgetPolicy::default(),
            placement: PlacementPolicy::default(),
            idle: IdlePolicy::default(),
            freshness: FreshnessPolicy::default(),
            limits: SafetyLimits::default(),
            local_hourly_cost_usd: None,
            rules: Vec::new(),
            metadata: BTreeMap::new(),
        }
    }
}
