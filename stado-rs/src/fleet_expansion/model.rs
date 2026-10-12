//! Declared investment assumptions and the reports derived from them.
use super::constants::SCHEMA_VERSION;
use crate::fleet_needs::Need;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Catalog {
    pub schema_version: u32,
    pub options: Vec<ExpansionOption>,
}

impl Default for Catalog {
    fn default() -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            options: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OptionKind {
    Buy,
    Upgrade,
    Rent,
    Reclaim,
    Relocate,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExpansionOption {
    pub id: String,
    pub label: String,
    pub kind: OptionKind,
    pub need_keys: Vec<String>,
    pub benefit_group: String,
    pub upfront_usd: Option<f64>,
    pub monthly_cost_usd: Option<f64>,
    pub monthly_savings_usd: Option<f64>,
    pub monthly_margin_usd: Option<f64>,
    pub lead_time_days: u32,
    pub evidence: String,
    pub observed_at: String,
    pub valid_until: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CatalogRecord {
    pub version: Option<String>,
    pub catalog: Catalog,
}

/// Where one option stands in a plan.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CandidateStatus {
    /// Evidenced, complete, within budget and with a positive gain.
    Eligible,
    /// Evidenced and complete, refused for the reasons listed.
    Excluded,
    /// A money field is unknown, so nothing could be computed.
    Unknown,
    /// Its evidence was observed in the future or has expired.
    Unevidenced,
}

/// What a plan concluded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlanStatus {
    Ready,
    Partial,
    InsufficientEvidence,
    NoViableOption,
    NoNeeds,
}

impl PlanStatus {
    /// The plan command exits zero: a portfolio covers every need, or there
    /// is no need to cover.
    pub fn succeeded(self) -> bool {
        match self {
            Self::Ready | Self::NoNeeds => true,
            Self::Partial | Self::InsufficientEvidence | Self::NoViableOption => false,
        }
    }
}

fn serde_word<T: Serialize>(
    value: &T,
    formatter: &mut std::fmt::Formatter<'_>,
) -> std::fmt::Result {
    match serde_json::to_value(value) {
        Ok(serde_json::Value::String(word)) => formatter.write_str(&word),
        _ => Err(std::fmt::Error),
    }
}

impl std::fmt::Display for CandidateStatus {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        serde_word(self, formatter)
    }
}

impl std::fmt::Display for PlanStatus {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        serde_word(self, formatter)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Candidate {
    #[serde(flatten)]
    pub option: ExpansionOption,
    pub status: CandidateStatus,
    pub reasons: Vec<String>,
    pub committed_cost_usd: Option<f64>,
    pub monthly_net_usd: Option<f64>,
    pub payback_months: Option<f64>,
    pub horizon_net_usd: Option<f64>,
    pub roi_pct: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Portfolio {
    pub selected_ids: Vec<String>,
    pub upfront_usd: f64,
    pub committed_cost_usd: f64,
    pub remaining_budget_usd: f64,
    pub monthly_net_usd: f64,
    pub horizon_net_usd: f64,
    pub payback_months: Option<f64>,
    pub roi_pct: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExpansionReport {
    pub schema_version: u32,
    pub plan_id: String,
    pub generated_at: String,
    pub stado_version: String,
    pub catalog_version: Option<String>,
    pub budget_usd: f64,
    pub horizon_months: u32,
    /// Days of refusals the plan read; None when every retained refusal.
    #[serde(default)]
    pub window_days: Option<i64>,
    pub status: PlanStatus,
    pub needs: Vec<Need>,
    pub candidates: Vec<Candidate>,
    pub portfolio: Portfolio,
    pub warnings: Vec<String>,
}

pub fn need_key(need: &Need) -> String {
    format!(
        "{}:{}",
        need.need.as_str(),
        need.target
            .as_deref()
            .or(need.platform.as_deref())
            .unwrap_or("fleet")
    )
}
