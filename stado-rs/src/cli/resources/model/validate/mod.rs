//! Everything a plan has to satisfy before anyone is allowed to act on it:
//! its canonical bytes and digest, the uniqueness of its ids, and, for every
//! action, the intent that admits it, the authorization it carries, the
//! reversibility its kind implies, the locator it addresses, the undo it
//! promises, and the findings and actions it depends on.

mod locator;
mod rollback;

use std::collections::BTreeSet;

use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::cli::CmdError;

use super::{ActionKind, Authorization, Intent, Plan, ProviderKind, Reversibility};
use locator::validate_action_locator;
use rollback::{rollback_pair, validate_rollback};

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
        if self.operation_id.is_empty() {
            return Err(CmdError::refused("resource plan needs an operation id"));
        }
        let finding_ids: BTreeSet<&str> =
            self.findings.iter().map(|item| item.id.as_str()).collect();
        if finding_ids.len() != self.findings.len() {
            return Err(CmdError::refused(
                "resource plan contains duplicate finding ids",
            ));
        }
        let action_ids: BTreeSet<&str> = self.actions.iter().map(|item| item.id.as_str()).collect();
        if action_ids.len() != self.actions.len() {
            return Err(CmdError::refused(
                "resource plan contains duplicate action ids",
            ));
        }
        for action in &self.actions {
            if action.id.is_empty()
                || action.resource.name.is_empty()
                || action.resource.reference.is_empty()
            {
                return Err(CmdError::refused(
                    "resource plan contains an action with an empty identity",
                ));
            }
            if action.resource.provider == ProviderKind::Gcp
                && action.resource.project.as_deref().is_none_or(str::is_empty)
            {
                return Err(CmdError::refused(format!(
                    "GCP action {} needs an explicit project id",
                    action.id
                )));
            }
            if !action.parameters.is_object()
                || action.preconditions.is_empty()
                || action.postconditions.is_empty()
            {
                return Err(CmdError::refused(format!(
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
                | ActionKind::StartInstance
                | ActionKind::SuspendCloudSql => Reversibility::Reversible,
                rollback => {
                    return Err(CmdError::refused(format!(
                        "rollback-only action kind {rollback:?} cannot appear in a plan"
                    )))
                }
            };
            if action.reversibility != expected_reversibility {
                return Err(CmdError::refused(format!(
                    "action {} has incorrect reversibility for {:?}",
                    action.id, action.kind
                )));
            }
            if !action.kind.allowed_for(self.intent) {
                return Err(CmdError::refused(format!(
                    "action {} is not allowed for {:?}",
                    action.id, self.intent
                )));
            }
            if self.intent == Intent::RationalizationCleanup
                && action.kind != ActionKind::DeleteInstance
                && action.authorization != Authorization::Explicit
            {
                return Err(CmdError::refused(format!(
                    "rationalization action {} requires explicit authorization",
                    action.id
                )));
            }
            if self.intent == Intent::AutonomousReconcile {
                let ownership = action
                    .parameters
                    .get("ownership")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                if action.authorization != Authorization::Automatic
                    || !matches!(ownership, "owned" | "adopted")
                {
                    return Err(CmdError::refused(format!(
                        "autonomous action {} requires automatic authorization and owned/adopted ownership",
                        action.id
                    )));
                }
            }
            if self.intent == Intent::Shutdown && action.authorization != Authorization::Automatic {
                return Err(CmdError::refused(format!(
                    "shutdown action {} has an invalid authorization mode",
                    action.id
                )));
            }
            if action.reversibility == Reversibility::Irreversible && action.rollback.is_some() {
                return Err(CmdError::refused(format!(
                    "irreversible action {} cannot claim a rollback",
                    action.id
                )));
            }
            if action.reversibility != Reversibility::Irreversible && action.rollback.is_none() {
                return Err(CmdError::refused(format!(
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
                    return Err(CmdError::refused(format!(
                        "action {} has invalid rollback metadata",
                        action.id
                    )));
                }
            }
            if self.intent == Intent::Shutdown
                && (action.reversibility == Reversibility::Irreversible
                    || action.rollback.is_none())
            {
                return Err(CmdError::refused(format!(
                    "shutdown action {} must be reversible",
                    action.id
                )));
            }
            if action
                .finding_id
                .as_deref()
                .is_some_and(|id| !finding_ids.contains(id))
            {
                return Err(CmdError::refused(format!(
                    "action {} references an unknown finding",
                    action.id
                )));
            }
            if action
                .depends_on
                .iter()
                .any(|dependency| !action_ids.contains(dependency.as_str()))
            {
                return Err(CmdError::refused(format!(
                    "action {} references an unknown dependency",
                    action.id
                )));
            }
            let dependencies: BTreeSet<&str> =
                action.depends_on.iter().map(String::as_str).collect();
            if dependencies.len() != action.depends_on.len()
                || dependencies.contains(action.id.as_str())
            {
                return Err(CmdError::refused(format!(
                    "action {} has duplicate or self dependencies",
                    action.id
                )));
            }
        }
        Ok(())
    }
}
