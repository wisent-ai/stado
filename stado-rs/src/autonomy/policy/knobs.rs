//! The vocabulary a decision is judged with, and the dials it is judged by.
//!
//! `AutonomyMode` is how much the control plane is allowed to do at all and
//! `ActionRisk` is what a single action would cost if it went wrong. The rest
//! are the dials an operator sets per document: what may be spent, where a
//! workload may land, and how stale the inputs may be before a decision is
//! refused. Every field name here is a published document key.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::capabilities::ProviderId;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AutonomyMode {
    #[default]
    Report,
    EnforceSafe,
    EnforceOwned,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActionRisk {
    ReadOnly,
    Reversible,
    Destructive,
    FinancialCommitment,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct BudgetPolicy {
    pub hourly_usd: Option<f64>,
    pub daily_usd: Option<f64>,
    pub monthly_usd: Option<f64>,
    pub max_single_action_usd: Option<f64>,
    pub max_commitment_usd: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct PlacementPolicy {
    pub allowed_providers: BTreeSet<ProviderId>,
    pub allowed_regions: BTreeSet<String>,
    pub prefer_local: bool,
    pub allow_spot: bool,
    pub require_checkpoint_for_spot: bool,
    pub account_for_egress: bool,
}

impl Default for PlacementPolicy {
    fn default() -> Self {
        Self {
            allowed_providers: [
                ProviderId::Local,
                ProviderId::Gcp,
                ProviderId::Azure,
                ProviderId::Aws,
                ProviderId::Box,
                ProviderId::Vast,
            ]
            .into_iter()
            .collect(),
            allowed_regions: BTreeSet::new(),
            prefer_local: true,
            allow_spot: false,
            require_checkpoint_for_spot: true,
            account_for_egress: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct FreshnessPolicy {
    pub inventory_max_age_seconds: u64,
    pub pricing_max_age_seconds: u64,
}

impl Default for FreshnessPolicy {
    fn default() -> Self {
        Self {
            inventory_max_age_seconds: crate::monitor::billing::SECONDS_PER_MINUTE
                * (u8::BITS as u64 - (u16::BITS / u8::BITS) as u64 - true as u64),
            pricing_max_age_seconds: crate::monitor::billing::SECONDS_PER_HOUR,
        }
    }
}
