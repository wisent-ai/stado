//! Versioned, provider-neutral resource operation model.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use super::super::CmdError;

pub const SCHEMA_VERSION: u8 = true as u8;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Intent {
    RationalizationCleanup,
    Shutdown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderKind {
    Stado,
    Local,
    Gcp,
    Azure,
    Aws,
    Vast,
    Box,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FindingDisposition {
    Automatic,
    ReviewRequired,
    Blocked,
    Unsupported,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Authorization {
    Automatic,
    Explicit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Reversibility {
    Reversible,
    SnapshotRestore,
    Irreversible,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActionKind {
    DeleteInstance,
    SnapshotDisk,
    DeleteDisk,
    ReleaseAddress,
    DeleteManagedInstanceGroup,
    ReleaseReservation,
    DisableStorageBackup,
    PauseScheduler,
    ResizeManagedInstanceGroup,
    StopInstance,
    SuspendCloudSql,
    DeleteSnapshot,
    RestoreDisk,
    EnableStorageBackup,
    ResumeScheduler,
    StartInstance,
    RestoreCloudSql,
}

impl ActionKind {
    pub fn allowed_for(self, intent: Intent) -> bool {
        match intent {
            Intent::RationalizationCleanup => matches!(
                self,
                Self::DeleteInstance
                    | Self::SnapshotDisk
                    | Self::DeleteDisk
                    | Self::ReleaseAddress
                    | Self::DeleteManagedInstanceGroup
                    | Self::ReleaseReservation
                    | Self::DisableStorageBackup
            ),
            Intent::Shutdown => matches!(
                self,
                Self::PauseScheduler
                    | Self::ResizeManagedInstanceGroup
                    | Self::StopInstance
                    | Self::SuspendCloudSql
            ),
        }
    }
}

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

impl Plan {
    pub fn canonical_bytes(&self) -> Result<Vec<u8>, CmdError> {
        let mut bytes = serde_json::to_vec_pretty(self)?;
        bytes.push(b'\n');
        Ok(bytes)
    }

    pub fn sha256(&self) -> Result<String, CmdError> {
        Ok(hex::encode(Sha256::digest(self.canonical_bytes()?)))
    }

    pub fn validate(&self) -> Result<(), CmdError> {
        if self.schema_version != SCHEMA_VERSION {
            return Err(CmdError::click(format!(
                "unsupported resource plan schema {}",
                self.schema_version
            )));
        }
        if self.operation_id.is_empty() {
            return Err(CmdError::click("resource plan needs an operation id"));
        }
        let finding_ids: BTreeSet<&str> =
            self.findings.iter().map(|item| item.id.as_str()).collect();
        if finding_ids.len() != self.findings.len() {
            return Err(CmdError::click(
                "resource plan contains duplicate finding ids",
            ));
        }
        let action_ids: BTreeSet<&str> = self.actions.iter().map(|item| item.id.as_str()).collect();
        if action_ids.len() != self.actions.len() {
            return Err(CmdError::click(
                "resource plan contains duplicate action ids",
            ));
        }
        for action in &self.actions {
            if action.id.is_empty()
                || action.resource.name.is_empty()
                || action.resource.reference.is_empty()
            {
                return Err(CmdError::click(
                    "resource plan contains an action with an empty identity",
                ));
            }
            if action.resource.provider == ProviderKind::Gcp
                && action.resource.project.as_deref().is_none_or(str::is_empty)
            {
                return Err(CmdError::click(format!(
                    "GCP action {} needs an explicit project id",
                    action.id
                )));
            }
            if !action.parameters.is_object()
                || action.preconditions.is_empty()
                || action.postconditions.is_empty()
            {
                return Err(CmdError::click(format!(
                    "action {} needs object parameters and explicit pre/postconditions",
                    action.id
                )));
            }
            validate_action_locator(action)?;
            let expected_reversibility = match action.kind {
                ActionKind::DeleteInstance
                | ActionKind::ReleaseAddress
                | ActionKind::DeleteManagedInstanceGroup
                | ActionKind::ReleaseReservation => Reversibility::Irreversible,
                ActionKind::DeleteDisk => Reversibility::SnapshotRestore,
                ActionKind::SnapshotDisk
                | ActionKind::DisableStorageBackup
                | ActionKind::PauseScheduler
                | ActionKind::ResizeManagedInstanceGroup
                | ActionKind::StopInstance
                | ActionKind::SuspendCloudSql => Reversibility::Reversible,
                rollback => {
                    return Err(CmdError::click(format!(
                        "rollback-only action kind {rollback:?} cannot appear in a plan"
                    )))
                }
            };
            if action.reversibility != expected_reversibility {
                return Err(CmdError::click(format!(
                    "action {} has incorrect reversibility for {:?}",
                    action.id, action.kind
                )));
            }
            if !action.kind.allowed_for(self.intent) {
                return Err(CmdError::click(format!(
                    "action {} is not allowed for {:?}",
                    action.id, self.intent
                )));
            }
            if self.intent == Intent::RationalizationCleanup
                && action.kind != ActionKind::DeleteInstance
                && action.authorization != Authorization::Explicit
            {
                return Err(CmdError::click(format!(
                    "rationalization action {} requires explicit authorization",
                    action.id
                )));
            }
            if self.intent == Intent::Shutdown && action.authorization != Authorization::Automatic {
                return Err(CmdError::click(format!(
                    "shutdown action {} has an invalid authorization mode",
                    action.id
                )));
            }
            if action.reversibility == Reversibility::Irreversible && action.rollback.is_some() {
                return Err(CmdError::click(format!(
                    "irreversible action {} cannot claim a rollback",
                    action.id
                )));
            }
            if action.reversibility != Reversibility::Irreversible && action.rollback.is_none() {
                return Err(CmdError::click(format!(
                    "reversible action {} needs rollback metadata",
                    action.id
                )));
            }
            if let Some(rollback) = &action.rollback {
                validate_rollback(action, rollback)?;
                if !rollback_pair(action.kind, rollback.kind)
                    || !rollback.parameters.is_object()
                    || rollback.preconditions.is_empty()
                    || rollback.postconditions.is_empty()
                {
                    return Err(CmdError::click(format!(
                        "action {} has invalid rollback metadata",
                        action.id
                    )));
                }
            }
            if self.intent == Intent::Shutdown
                && (action.reversibility == Reversibility::Irreversible
                    || action.rollback.is_none())
            {
                return Err(CmdError::click(format!(
                    "shutdown action {} must be reversible",
                    action.id
                )));
            }
            if action
                .finding_id
                .as_deref()
                .is_some_and(|id| !finding_ids.contains(id))
            {
                return Err(CmdError::click(format!(
                    "action {} references an unknown finding",
                    action.id
                )));
            }
            if action
                .depends_on
                .iter()
                .any(|dependency| !action_ids.contains(dependency.as_str()))
            {
                return Err(CmdError::click(format!(
                    "action {} references an unknown dependency",
                    action.id
                )));
            }
            let dependencies: BTreeSet<&str> =
                action.depends_on.iter().map(String::as_str).collect();
            if dependencies.len() != action.depends_on.len()
                || dependencies.contains(action.id.as_str())
            {
                return Err(CmdError::click(format!(
                    "action {} has duplicate or self dependencies",
                    action.id
                )));
            }
        }
        Ok(())
    }
}

fn validate_action_locator(action: &Action) -> Result<(), CmdError> {
    let valid = match action.kind {
        ActionKind::DeleteInstance => {
            action.resource.resource_type == "agent-vm"
                && matches!(
                    action.resource.provider,
                    ProviderKind::Gcp | ProviderKind::Azure | ProviderKind::Aws | ProviderKind::Box
                )
        }
        ActionKind::SnapshotDisk | ActionKind::DeleteDisk => {
            valid_gcp_locator(action, "persistent-disk", &["zone", "region"])
                && action
                    .parameters
                    .get("snapshot_name")
                    .and_then(Value::as_str)
                    .is_some_and(valid_component)
        }
        ActionKind::ReleaseAddress => {
            valid_gcp_locator(action, "static-address", &["global", "region"])
        }
        ActionKind::DeleteManagedInstanceGroup => {
            valid_gcp_locator(action, "managed-instance-group", &["zone", "region"])
        }
        ActionKind::ResizeManagedInstanceGroup => {
            valid_gcp_locator(action, "managed-instance-group", &["zone", "region"])
                && action.parameters.get("target_size").and_then(Value::as_i64)
                    == Some(i64::default())
        }
        ActionKind::ReleaseReservation => {
            valid_gcp_locator(action, "compute-reservation", &["zone"])
        }
        ActionKind::DisableStorageBackup => {
            action.resource.provider == ProviderKind::Stado
                && action.resource.resource_type == "storage-backup"
                && action
                    .parameters
                    .get("previous")
                    .is_some_and(Value::is_object)
        }
        ActionKind::PauseScheduler => valid_gcp_locator(action, "scheduler-job", &["region"]),
        ActionKind::StopInstance => valid_gcp_locator(action, "instance", &["zone"]),
        ActionKind::SuspendCloudSql => valid_gcp_locator(action, "cloud-sql-instance", &["global"]),
        rollback => {
            return Err(CmdError::click(format!(
                "rollback-only action kind {rollback:?} cannot appear in a plan"
            )))
        }
    };
    if !valid {
        return Err(CmdError::click(format!(
            "action {} has a provider, type, scope, or locator incompatible with {:?}",
            action.id, action.kind
        )));
    }
    Ok(())
}

fn valid_gcp_locator(action: &Action, resource_type: &str, scopes: &[&str]) -> bool {
    let scope = action
        .parameters
        .get("scope")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let needs_location = scope != "global";
    action.resource.provider == ProviderKind::Gcp
        && action.resource.resource_type == resource_type
        && action
            .resource
            .project
            .as_deref()
            .is_some_and(valid_component)
        && valid_component(&action.resource.name)
        && scopes.contains(&scope)
        && (!needs_location
            || action
                .resource
                .location
                .as_deref()
                .is_some_and(valid_component))
}

fn valid_component(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= u8::MAX as usize
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
}

fn validate_rollback(action: &Action, rollback: &Rollback) -> Result<(), CmdError> {
    let valid = match (action.kind, rollback.kind) {
        (ActionKind::SnapshotDisk, ActionKind::DeleteSnapshot) => {
            let planned = action
                .parameters
                .get("snapshot_name")
                .and_then(Value::as_str);
            let restored = rollback
                .parameters
                .get("snapshot_name")
                .and_then(Value::as_str);
            planned == restored && planned.is_some_and(valid_component)
        }
        (ActionKind::DeleteDisk, ActionKind::RestoreDisk) => {
            let planned = action
                .parameters
                .get("snapshot_name")
                .and_then(Value::as_str);
            let restored = rollback
                .parameters
                .get("snapshot_name")
                .and_then(Value::as_str);
            planned == restored
                && planned.is_some_and(valid_component)
                && action.parameters.get("scope") == rollback.parameters.get("scope")
                && action
                    .parameters
                    .get("original")
                    .is_some_and(Value::is_object)
                && action.parameters.get("original") == rollback.parameters.get("original")
        }
        (ActionKind::DisableStorageBackup, ActionKind::EnableStorageBackup) => rollback
            .parameters
            .get("backup")
            .is_some_and(|value| value.is_object()),
        (ActionKind::PauseScheduler, ActionKind::ResumeScheduler)
        | (ActionKind::StopInstance, ActionKind::StartInstance) => rollback
            .parameters
            .as_object()
            .is_some_and(serde_json::Map::is_empty),
        (ActionKind::ResizeManagedInstanceGroup, ActionKind::ResizeManagedInstanceGroup) => {
            rollback
                .parameters
                .get("target_size")
                .and_then(Value::as_i64)
                .is_some_and(|size| size >= i64::default())
                && rollback.parameters.get("scope") == action.parameters.get("scope")
        }
        (ActionKind::SuspendCloudSql, ActionKind::RestoreCloudSql) => rollback
            .parameters
            .get("activation_policy")
            .and_then(Value::as_str)
            .is_some_and(|policy| {
                !policy.is_empty()
                    && policy.len() <= u8::MAX as usize
                    && policy
                        .bytes()
                        .all(|byte| byte.is_ascii_uppercase() || byte == b'_')
            }),
        _ => false,
    };
    if !valid {
        return Err(CmdError::click(format!(
            "action {} has unsafe rollback parameters",
            action.id
        )));
    }
    Ok(())
}

fn rollback_pair(action: ActionKind, rollback: ActionKind) -> bool {
    matches!(
        (action, rollback),
        (ActionKind::SnapshotDisk, ActionKind::DeleteSnapshot)
            | (ActionKind::DeleteDisk, ActionKind::RestoreDisk)
            | (
                ActionKind::DisableStorageBackup,
                ActionKind::EnableStorageBackup
            )
            | (ActionKind::PauseScheduler, ActionKind::ResumeScheduler)
            | (
                ActionKind::ResizeManagedInstanceGroup,
                ActionKind::ResizeManagedInstanceGroup
            )
            | (ActionKind::StopInstance, ActionKind::StartInstance)
            | (ActionKind::SuspendCloudSql, ActionKind::RestoreCloudSql)
    )
}

pub fn canonical_json_bytes<T: Serialize>(value: &T) -> Result<Vec<u8>, CmdError> {
    let mut bytes = serde_json::to_vec_pretty(value)?;
    bytes.push(b'\n');
    Ok(bytes)
}
