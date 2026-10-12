//! The inventory family: what a resource is, who owns it, and how it links.
//!
//! [`Ownership`]
//! and [`SourceState`] are the vocabulary the read uses, [`ResourceRecord`] is
//! one observed resource with its canonical identity, and
//! [`canonical_resource_id`] mints that identity. [`InventorySource`] is one
//! provider account's read, [`InventorySnapshot`] the sealed set of them, and
//! [`ResourceGraph`] the dependency edges between the resources in it.

use std::collections::{BTreeMap, BTreeSet};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::capabilities::ProviderId;
use crate::providers::LifecyclePhase;

use super::ResourceKind;

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
    pub resource_id: String,
    pub provider: ProviderId,
    pub account: String,
    pub region: Option<String>,
    pub zone: Option<String>,
    pub resource_type: ResourceKind,
    pub native_reference: String,
    pub name: String,
    /// The provider's own state word, kept for the record.
    pub state: String,
    /// What that word means, mapped once by the source that read it. A
    /// snapshot written before the field existed reads as unknown.
    #[serde(default)]
    pub phase: LifecyclePhase,
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
        resource_type: ResourceKind,
        native_reference: impl Into<String>,
        name: impl Into<String>,
        observed_at: DateTime<Utc>,
    ) -> Self {
        let account = account.into();
        let native_reference = native_reference.into();
        Self {
            resource_id: canonical_resource_id(
                provider,
                &account,
                &resource_type.to_string(),
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
            phase: LifecyclePhase::Unknown,
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
        self.owner = identity_label(&self.labels, labels::OWNER);
        self.workload = identity_label(&self.labels, labels::WORKLOAD);
        self.environment = identity_label(&self.labels, labels::ENVIRONMENT);
        self.managed_by = identity_label(&self.labels, labels::MANAGED_BY);
        self.ownership = if self.managed_by.as_deref() == Some(labels::MANAGED_BY_STADO) {
            Ownership::Owned
        } else if labels::set(&self.labels, labels::ADOPTED) {
            Ownership::Adopted
        } else if self.owner.is_some() {
            Ownership::Observed
        } else {
            Ownership::Unknown
        };
    }
}

/// The labels (tags on AWS and Azure) that say who a resource belongs to.
/// Each field is read from exactly one label; a resource carrying another
/// spelling has no value for that field.
pub mod labels {
    use std::collections::BTreeMap;

    /// The person or team that owns the resource.
    pub const OWNER: &str = "stado-owner";
    /// The workload the resource serves; a resource with one is never idle.
    pub const WORKLOAD: &str = "stado-workload";
    /// The environment, which production protection reads.
    pub const ENVIRONMENT: &str = "stado-environment";
    /// Who manages the resource; [`MANAGED_BY_STADO`] makes it Stado's own.
    pub const MANAGED_BY: &str = "managed-by";
    /// The value of [`MANAGED_BY`] on a resource Stado owns.
    pub const MANAGED_BY_STADO: &str = "stado";
    /// `true` on a resource an operator has brought under Stado.
    pub const ADOPTED: &str = "stado-adopted";
    /// `true` on a resource that holds data the policy must protect.
    pub const STATEFUL: &str = "stado-stateful";

    /// The label is present and reads as the boolean `true`.
    pub fn set(labels: &BTreeMap<String, String>, name: &str) -> bool {
        labels
            .get(name)
            .is_some_and(|value| value.trim().parse::<bool>() == Ok(true))
    }
}

fn identity_label(labels: &BTreeMap<String, String>, name: &str) -> Option<String> {
    labels
        .get(name)
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
    pub upstream_error: Option<String>,
    pub resources: Vec<ResourceRecord>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InventorySnapshot {
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
