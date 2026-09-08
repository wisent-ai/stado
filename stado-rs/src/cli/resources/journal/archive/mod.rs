//! The archive handle every phase drives: the storage it writes through, the
//! local mirror it duplicates into, the immutable plan and initial state one
//! operation is created with, and the conflict message its compare-and-swap
//! mutations share.
//!
//! `load` reads the archived documents back and swaps state, `lease` holds the
//! single-writer lock a mutation is made under, `trail` appends the events and
//! artifacts a transition publishes, and `command` serves the read-only
//! operation history subcommands.

mod command;
mod lease;
mod load;
mod trail;

use std::path::PathBuf;

use serde_json::json;

use crate::cli::resources::journal::clock::now;
use crate::cli::resources::journal::names::{remote_path, validate_operation_id};
use crate::cli::resources::journal::records::{ActionPhase, ActionState, OperationState, Phase};
use crate::cli::resources::model::{canonical_json_bytes, Plan, SCHEMA_VERSION};
use crate::cli::CmdError;
use crate::queue::{JobStorage, StorageError};

pub use command::dispatch;

pub struct Journal {
    store: JobStorage,
    local_root: PathBuf,
}

impl Journal {
    pub async fn open() -> Result<Self, CmdError> {
        let store = JobStorage::new().await?;
        let home = std::env::var_os("HOME").ok_or_else(|| CmdError::click("HOME is not set"))?;
        let local_root = PathBuf::from(home).join(".stado").join("operations");
        Ok(Self { store, local_root })
    }

    pub async fn create(&self, plan: &Plan) -> Result<OperationState, CmdError> {
        plan.validate()?;
        validate_operation_id(&plan.operation_id)?;
        let bytes = plan.canonical_bytes()?;
        let text =
            String::from_utf8(bytes.clone()).map_err(|error| CmdError::click(error.to_string()))?;
        let plan_path = remote_path(&plan.operation_id, "plan.json");
        if !self.store.create_text_if_absent(&plan_path, &text).await? {
            let existing = self.store.download_text(&plan_path).await?.ok_or_else(|| {
                CmdError::click("operation plan disappeared after create conflict")
            })?;
            if existing.as_bytes() != bytes {
                return Err(CmdError::click(format!(
                    "operation {} already has a different immutable plan",
                    plan.operation_id
                )));
            }
        }
        self.write_local(&plan.operation_id, "plan.json", &bytes)?;

        let created_at = now();
        let actions = plan
            .actions
            .iter()
            .map(|action| {
                (
                    action.id.clone(),
                    ActionState {
                        action_id: action.id.clone(),
                        kind: action.kind,
                        phase: ActionPhase::Pending,
                        observed_before: None,
                        observed_after: None,
                        receipt: None,
                        error: None,
                        updated_at: created_at.clone(),
                    },
                )
            })
            .collect();
        let state = OperationState {
            schema_version: SCHEMA_VERSION,
            operation_id: plan.operation_id.clone(),
            plan_hash: plan.sha256()?,
            phase: Phase::Planned,
            revision: u64::default(),
            created_at: created_at.clone(),
            updated_at: created_at,
            actions,
            error: None,
        };
        let body = String::from_utf8(canonical_json_bytes(&state)?)
            .map_err(|error| CmdError::click(error.to_string()))?;
        let state_path = remote_path(&plan.operation_id, "state.json");
        if !self.store.create_text_if_absent(&state_path, &body).await? {
            let existing = self.load_state(&plan.operation_id).await?;
            if existing.plan_hash != state.plan_hash {
                return Err(CmdError::click(
                    "existing operation state references a different plan hash",
                ));
            }
            let existing_bytes = canonical_json_bytes(&existing)?;
            self.write_local(&plan.operation_id, "state.json", &existing_bytes)?;
            return Ok(existing);
        }
        self.write_local(&plan.operation_id, "state.json", body.as_bytes())?;
        self.event(
            &plan.operation_id,
            "planned",
            None,
            json!({"plan_hash": state.plan_hash, "actions": state.actions.len()}),
        )
        .await?;
        Ok(state)
    }
}

fn map_conflict(error: StorageError) -> CmdError {
    match error {
        StorageError::StorageConflict(_) => CmdError::click(
            "operation state changed concurrently; inspect it before deciding whether to resume",
        ),
        other => CmdError::click(other.to_string()),
    }
}
