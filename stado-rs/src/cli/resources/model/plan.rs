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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SourceSnapshot {
    pub name: String,
    pub state: String,
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
    pub schema_version: u8,
    pub operation_id: String,
    pub intent: Intent,
    pub created_at: String,
    pub expires_at: String,
    pub stado_version: String,
    pub scope: OperationScope,
    pub configuration_fingerprint: String,
    pub inventory: InventorySnapshot,
    pub findings: Vec<Finding>,
    pub actions: Vec<Action>,
}
