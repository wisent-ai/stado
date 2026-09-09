//! The decision family: what a pass decided and what that decision returned.
//!
//! [`DecisionKind`] and [`DecisionRecord`] are the decision itself.
//! [`SavingsRecord`] is the predicted economics of one decision,
//! [`SavingsMeasurement`] the later observation that settles it, and
//! [`AdoptionRecord`] the note that a resource this plane did not create is
//! now managed by it.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::capabilities::ProviderId;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DecisionKind {
    Placement,
    Cleanup,
    Rightsize,
    Schedule,
    StorageLifecycle,
    Network,
    Commitment,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DecisionRecord {
    pub schema_version: u16,
    pub decision_id: String,
    pub kind: DecisionKind,
    pub subject_id: String,
    pub created_at: String,
    pub expires_at: String,
    pub inventory_snapshot_id: String,
    pub policy_version: String,
    pub selected: Option<Value>,
    pub candidates: Vec<Value>,
    pub constraints: Vec<String>,
    pub explanation: String,
    pub lease_token: Option<String>,
    pub state: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SavingsRecord {
    pub schema_version: u16,
    pub savings_id: String,
    pub decision_id: String,
    pub resource_id: Option<String>,
    pub workload: Option<String>,
    pub provider: ProviderId,
    pub measurement_started_at: String,
    pub measurement_ended_at: Option<String>,
    pub baseline_cost_usd: f64,
    pub predicted_cost_usd: f64,
    pub realized_cost_usd: Option<f64>,
    pub predicted_savings_usd: f64,
    pub realized_savings_usd: Option<f64>,
    pub confidence: f64,
    pub source_invoice_period: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SavingsMeasurement {
    pub schema_version: u16,
    pub measurement_id: String,
    pub savings_id: String,
    pub decision_id: String,
    pub measured_at: String,
    pub realized_cost_usd: f64,
    pub realized_savings_usd: f64,
    pub source: String,
    pub source_invoice_period: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AdoptionRecord {
    pub schema_version: u16,
    pub resource_id: String,
    pub adopted_at: String,
    pub adopted_by: String,
    pub owner: String,
    pub policy_ref: String,
    pub source_revision: Option<String>,
}
