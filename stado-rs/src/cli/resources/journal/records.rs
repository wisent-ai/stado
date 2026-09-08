//! The documents one operation is archived as: the two phase vocabularies, the
//! recorded state of a single action, the operation state every mutation swaps,
//! the append-only event a transition writes beside it, and the one check a
//! state document has to pass before it is trusted.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::cli::resources::model::{ActionKind, SCHEMA_VERSION};
use crate::cli::CmdError;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    Planned,
    Preflighting,
    Applying,
    Applied,
    ApplyFailed,
    Verifying,
    Verified,
    Drifted,
    Restoring,
    Restored,
    RestoreFailed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActionPhase {
    Pending,
    Preflighted,
    Applying,
    Applied,
    AlreadyDesired,
    Skipped,
    Failed,
    Restoring,
    Restored,
    AlreadyRestored,
    Irreversible,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ActionState {
    pub action_id: String,
    pub kind: ActionKind,
    pub phase: ActionPhase,
    pub observed_before: Option<Value>,
    pub observed_after: Option<Value>,
    pub receipt: Option<Value>,
    pub error: Option<String>,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OperationState {
    pub schema_version: u8,
    pub operation_id: String,
    pub plan_hash: String,
    pub phase: Phase,
    pub revision: u64,
    pub created_at: String,
    pub updated_at: String,
    pub actions: BTreeMap<String, ActionState>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OperationEvent {
    pub schema_version: u8,
    pub event_id: String,
    pub operation_id: String,
    pub recorded_at: String,
    pub event: String,
    pub action_id: Option<String>,
    pub detail: Value,
}

pub(super) fn validate_state(operation_id: &str, state: &OperationState) -> Result<(), CmdError> {
    if state.schema_version != SCHEMA_VERSION || state.operation_id != operation_id {
        return Err(CmdError::click("invalid operation state document"));
    }
    Ok(())
}
