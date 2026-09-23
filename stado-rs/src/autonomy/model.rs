//! Provider-neutral contracts for autonomous resource and cost management.

use std::collections::{BTreeMap, BTreeSet};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::capabilities::ProviderId;

pub const SCHEMA_VERSION: u16 = true as u16;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Ownership {
    Owned,
    Adopted,
    Observed,
    Unknown,
}

impl Ownership {
    pub const fn is_mutable(self) -> bool {
        matches!(self, Self::Owned | Self::Adopted)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceState {
    Complete,
    Degraded,
    Blocked,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ResourceRecord {
    pub schema_version: u16,
    pub resource_id: String,
    pub provider: ProviderId,
    pub account: String,
    pub region: Option<String>,
    pub zone: Option<String>,
    pub resource_type: String,
    pub native_reference: String,
    pub name: String,
    pub state: String,
    pub created_at: Option<String>,
    pub last_seen_at: String,
    pub owner: Option<String>,
    pub workload: Option<String>,
    pub environment: Option<String>,
    pub managed_by: Option<String>,
    pub ownership: Ownership,
    #[serde(default)]
    pub labels: BTreeMap<String, String>,
    #[serde(default)]
    pub dependencies: BTreeSet<String>,
    #[serde(default)]
    pub utilization: BTreeMap<String, f64>,
    pub current_hourly_cost_usd: Option<f64>,
    pub forecast_monthly_cost_usd: Option<f64>,
    pub policy_ref: Option<String>,
    pub source_revision: Option<String>,
    #[serde(default)]
    pub evidence: Value,
}

impl ResourceRecord {
    pub fn new(
        provider: ProviderId,
        account: impl Into<String>,
        resource_type: impl Into<String>,
        native_reference: impl Into<String>,
        name: impl Into<String>,
        observed_at: DateTime<Utc>,
    ) -> Self {
        let account = account.into();
        let resource_type = resource_type.into();
        let native_reference = native_reference.into();
        Self {
            schema_version: SCHEMA_VERSION,
            resource_id: canonical_resource_id(
                provider,
                &account,
                &resource_type,
                &native_reference,
            ),
            provider,
            account,
            region: None,
            zone: None,
            resource_type,
            native_reference,
            name: name.into(),
            state: "unknown".to_string(),
            created_at: None,
            last_seen_at: observed_at.to_rfc3339(),
            owner: None,
            workload: None,
            environment: None,
            managed_by: None,
            ownership: Ownership::Unknown,
            labels: BTreeMap::new(),
            dependencies: BTreeSet::new(),
            utilization: BTreeMap::new(),
            current_hourly_cost_usd: None,
            forecast_monthly_cost_usd: None,
            policy_ref: None,
            source_revision: None,
            evidence: Value::Null,
        }
    }

    pub fn apply_identity_labels(&mut self) {
        self.owner = identity_label(&self.labels, &["stado-owner", "owner"]);
        self.workload = identity_label(&self.labels, &["stado-workload", "workload", "job_id"]);
        self.environment =
            identity_label(&self.labels, &["stado-environment", "environment", "env"]);
        self.managed_by = identity_label(
            &self.labels,
            &["managed-by", "managed_by", "stado-managed-by"],
        );
        self.ownership = match self.managed_by.as_deref() {
            Some("stado") | Some("wisent-compute") => Ownership::Owned,
            _ if self
                .labels
                .get("stado-adopted")
                .is_some_and(|value| value == "true") =>
            {
                Ownership::Adopted
            }
            _ if self.owner.is_some() => Ownership::Observed,
            _ => Ownership::Unknown,
        };
    }
}

fn identity_label(labels: &BTreeMap<String, String>, names: &[&str]) -> Option<String> {
    names
        .iter()
        .find_map(|name| labels.get(*name))
        .filter(|value| !value.trim().is_empty())
        .cloned()
}

pub fn canonical_resource_id(
    provider: ProviderId,
    account: &str,
    resource_type: &str,
    native_reference: &str,
) -> String {
    format!(
        "stado://{}/{}/{}/{}",
        provider.as_str(),
        encode_component(account),
        encode_component(resource_type),
        encode_component(native_reference)
    )
}

fn encode_component(value: &str) -> String {
    url::form_urlencoded::byte_serialize(value.as_bytes()).collect()
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InventorySource {
    pub provider: ProviderId,
    pub account: String,
    pub state: SourceState,
    pub observed_at: String,
    pub coverage: BTreeSet<String>,
    #[serde(default)]
    pub missing_permissions: Vec<String>,
    pub upstream_error: Option<String>,
    pub resources: Vec<ResourceRecord>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InventorySnapshot {
    pub schema_version: u16,
    pub snapshot_id: String,
    pub created_at: String,
    pub complete: bool,
    pub sources: Vec<InventorySource>,
    pub resources: Vec<ResourceRecord>,
    pub graph: ResourceGraph,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ResourceGraph {
    pub dependencies: BTreeMap<String, BTreeSet<String>>,
    pub dependents: BTreeMap<String, BTreeSet<String>>,
}

impl ResourceGraph {
    pub fn from_resources(resources: &[ResourceRecord]) -> Self {
        let known: BTreeSet<&str> = resources
            .iter()
            .map(|resource| resource.resource_id.as_str())
            .collect();
        let mut graph = Self::default();
        for resource in resources {
            graph
                .dependencies
                .entry(resource.resource_id.clone())
                .or_default();
            graph
                .dependents
                .entry(resource.resource_id.clone())
                .or_default();
            for dependency in &resource.dependencies {
                if !known.contains(dependency.as_str()) {
                    continue;
                }
                graph
                    .dependencies
                    .entry(resource.resource_id.clone())
                    .or_default()
                    .insert(dependency.clone());
                graph
                    .dependents
                    .entry(dependency.clone())
                    .or_default()
                    .insert(resource.resource_id.clone());
            }
        }
        graph
    }
}

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
