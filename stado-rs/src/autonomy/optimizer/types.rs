//! The placement records one pass carries: offers in, candidates out.
//!
//! [`PlacementCandidate`] is the priced, constraint-checked target the pass
//! writes into a decision and [`PlacementRunSummary`] counts what the pass
//! did. [`CapacityOffer`] is a unit of capacity the offer read found,
//! [`CloudBudget`] the remaining headroom for new cloud capacity, and
//! [`CandidateContext`] what the scoring needs besides the offer itself.

use std::collections::BTreeMap;

use chrono::Utc;
use serde::{Deserialize, Serialize};

use crate::autonomy::policy::AutonomyPolicy;
use crate::capabilities::ProviderId;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlacementCandidate {
    pub target_id: String,
    pub provider: ProviderId,
    pub region: Option<String>,
    pub machine_type: String,
    pub accelerator_type: String,
    pub vram_gb: i64,
    pub available_instances: i64,
    pub existing_capacity: bool,
    pub preemptible: bool,
    pub startup_seconds: f64,
    pub runtime_seconds: f64,
    pub hourly_compute_usd: Option<f64>,
    pub compute_cost_usd: Option<f64>,
    pub storage_cost_usd: f64,
    pub egress_cost_usd: Option<f64>,
    pub retry_risk_cost_usd: Option<f64>,
    pub slo_penalty_usd: f64,
    pub expected_total_cost_usd: Option<f64>,
    pub expected_finish_seconds: f64,
    pub price_source: Option<String>,
    pub eligible: bool,
    pub rejected_reasons: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PlacementRunSummary {
    pub considered_jobs: usize,
    pub decided_jobs: usize,
    pub changed_jobs: usize,
    pub no_eligible_target: usize,
    pub active_lease_skips: usize,
    pub provider_errors: BTreeMap<String, String>,
}

#[derive(Debug, Clone)]
pub(super) struct CapacityOffer {
    pub(super) target_id: String,
    pub(super) provider: ProviderId,
    pub(super) region: Option<String>,
    pub(super) accelerator_type: String,
    pub(super) machine_type: String,
    pub(super) free_vram_gb: i64,
    pub(super) available_instances: i64,
    pub(super) existing: bool,
}

#[derive(Debug, Clone, Copy)]
pub(super) struct CloudBudget {
    pub(super) hourly_usd: Option<f64>,
    pub(super) total_usd: Option<f64>,
}

pub(super) struct CandidateContext<'a> {
    pub(super) policy: &'a AutonomyPolicy,
    pub(super) feedback: &'a [super::storage::PlacementFeedback],
    pub(super) budget: CloudBudget,
    pub(super) now: chrono::DateTime<Utc>,
}
