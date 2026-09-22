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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Candidate {
    #[serde(flatten)]
    pub option: ExpansionOption,
    pub status: String,
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
    pub window_days: i64,
    pub status: String,
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
