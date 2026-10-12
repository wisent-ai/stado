//! The shape of an operation on the wire: where a resource lives, the
//! conditions an action is bracketed by, the findings that motivate it, the
//! inventory snapshot it is bound to, and the plan that carries all of them.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{ActionKind, Authorization, FindingDisposition, Intent, ProviderKind, Reversibility};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResourceLocator {
    pub provider: ProviderKind,
    pub resource_type: String,
    pub project: Option<String>,
    pub location: Option<String>,
    pub name: String,
    pub reference: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Condition {
    pub field: String,
    pub expected: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Rollback {
    pub kind: ActionKind,
    pub parameters: Value,
    pub preconditions: Vec<Condition>,
    pub postconditions: Vec<Condition>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Finding {
    pub id: String,
    pub severity: String,
    pub confidence: String,
    pub recommendation: String,
    pub reason: String,
    pub evidence: Value,
    pub disposition: FindingDisposition,
    pub resource: ResourceLocator,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Action {
    pub id: String,
    pub finding_id: Option<String>,
    pub kind: ActionKind,
    pub authorization: Authorization,
    pub reversibility: Reversibility,
    pub resource: ResourceLocator,
    pub parameters: Value,
    pub preconditions: Vec<Condition>,
    pub postconditions: Vec<Condition>,
    pub rollback: Option<Rollback>,
    pub depends_on: Vec<String>,
}

/// How completely one source of an inventory or a plan was read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceCoverage {
    /// Read in full.
    Ok,
    /// Not configured, so there was nothing to read.
    Skipped,
    /// Could not be read at all.
    Blocked,
    /// Read, with parts missing or failing.
    Degraded,
    /// This build cannot read it, so it is not covered.
    Unsupported,
}

impl SourceCoverage {
    /// The source leaves the report complete: read in full, or not
    /// configured.
    pub fn complete(self) -> bool {
        match self {
            Self::Ok | Self::Skipped => true,
            Self::Blocked | Self::Degraded | Self::Unsupported => false,
        }
    }
}

impl From<crate::providers::gcp::inventory::InventoryState> for SourceCoverage {
    fn from(state: crate::providers::gcp::inventory::InventoryState) -> Self {
        match state {
            crate::providers::gcp::inventory::InventoryState::Healthy => Self::Ok,
            crate::providers::gcp::inventory::InventoryState::Degraded => Self::Degraded,
            crate::providers::gcp::inventory::InventoryState::Critical => Self::Blocked,
        }
    }
}

impl From<crate::autonomy::model::SourceState> for SourceCoverage {
    fn from(state: crate::autonomy::model::SourceState) -> Self {
        match state {
            crate::autonomy::model::SourceState::Complete => Self::Ok,
            crate::autonomy::model::SourceState::Degraded => Self::Degraded,
            crate::autonomy::model::SourceState::Blocked => Self::Blocked,
        }
    }
}

impl std::fmt::Display for SourceCoverage {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match serde_json::to_value(self) {
            Ok(Value::String(word)) => formatter.write_str(&word),
            _ => Err(std::fmt::Error),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SourceSnapshot {
    pub name: String,
    pub state: SourceCoverage,
    pub detail: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InventorySnapshot {
    pub snapshot_id: String,
    pub complete: bool,
    pub sources: Vec<SourceSnapshot>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OperationScope {
    pub providers: BTreeSet<ProviderKind>,
    pub projects: BTreeSet<String>,
    pub storage: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Plan {
    pub operation_id: String,
    pub intent: Intent,
    pub created_at: String,
    /// Set only by the autonomy reconciler, from the policy's
    /// `decision_ttl_seconds`; an operator's plan has none.
    #[serde(default)]
    pub expires_at: Option<String>,
    pub stado_version: String,
    pub scope: OperationScope,
    pub configuration_fingerprint: String,
    pub inventory: InventorySnapshot,
    pub findings: Vec<Finding>,
    pub actions: Vec<Action>,
}
