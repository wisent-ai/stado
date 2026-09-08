//! Reading the archive back, and the only mutation path operation state has:
//! every write is a compare-and-swap against the exact version it was read at,
//! and every action the change touched is restamped by that same swap.

use crate::cli::resources::journal::clock::now;
use crate::cli::resources::journal::names::{remote_path, validate_operation_id};
use crate::cli::resources::journal::records::{validate_state, OperationState};
use crate::cli::resources::model::{canonical_json_bytes, Plan};
use crate::cli::CmdError;

use super::{map_conflict, Journal};

impl Journal {
    pub async fn load_plan(&self, operation_id: &str) -> Result<Plan, CmdError> {
        validate_operation_id(operation_id)?;
        let body = self
            .store
            .download_text(&remote_path(operation_id, "plan.json"))
            .await?
            .ok_or_else(|| CmdError::click(format!("operation {operation_id} has no plan")))?;
        let plan: Plan = serde_json::from_str(&body)?;
        plan.validate()?;
        if plan.operation_id != operation_id {
            return Err(CmdError::click("operation id does not match archived plan"));
        }
        if plan.canonical_bytes()? != body.as_bytes() {
            return Err(CmdError::click("archived plan is not canonical Stado JSON"));
        }
        Ok(plan)
    }

    pub async fn load_state(&self, operation_id: &str) -> Result<OperationState, CmdError> {
        validate_operation_id(operation_id)?;
        let body = self
            .store
            .download_text(&remote_path(operation_id, "state.json"))
            .await?
            .ok_or_else(|| CmdError::click(format!("operation {operation_id} has no state")))?;
        let state: OperationState = serde_json::from_str(&body)?;
        validate_state(operation_id, &state)?;
        Ok(state)
    }

    pub async fn update<F>(&self, operation_id: &str, change: F) -> Result<OperationState, CmdError>
    where
        F: FnOnce(&mut OperationState) -> Result<(), CmdError>,
    {
        validate_operation_id(operation_id)?;
        let path = remote_path(operation_id, "state.json");
        let versioned = self
            .store
            .read_text_versioned(&path)
            .await?
            .ok_or_else(|| CmdError::click(format!("operation {operation_id} has no state")))?;
        let mut state: OperationState = serde_json::from_str(&versioned.content)?;
        validate_state(operation_id, &state)?;
        let before_actions = state.actions.clone();
        change(&mut state)?;
        state.revision = state.revision.saturating_add(true as u64);
        let updated_at = now();
        for (action_id, action) in &mut state.actions {
            if before_actions.get(action_id) != Some(action) {
                action.updated_at = updated_at.clone();
            }
        }
        state.updated_at = updated_at;
        let body = String::from_utf8(canonical_json_bytes(&state)?)
            .map_err(|error| CmdError::click(error.to_string()))?;
        self.store
            .compare_and_swap_text(&path, &versioned.version, &body)
            .await
            .map_err(map_conflict)?;
        self.write_local(operation_id, "state.json", body.as_bytes())?;
        Ok(state)
    }
}
