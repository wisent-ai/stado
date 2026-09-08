//! The failed-transition writes: each one moves the operation and the action
//! it stopped at into their terminal failure phase and records the reason on
//! the journal before the error is returned to the caller.

use serde_json::{json, Value};

use crate::cli::resources::journal::{ActionPhase, Journal, Phase};
use crate::cli::resources::model::{Action, Plan};
use crate::cli::CmdError;

pub(super) async fn fail_preflight(
    journal: &Journal,
    plan: &Plan,
    action: &Action,
    observed: Option<&Value>,
    error: &str,
) -> Result<(), CmdError> {
    journal
        .update(&plan.operation_id, |state| {
            state.phase = Phase::ApplyFailed;
            state.error = Some(error.to_string());
            if let Some(item) = state.actions.get_mut(&action.id) {
                item.phase = ActionPhase::Failed;
                item.observed_before = observed.cloned();
                item.error = Some(error.to_string());
            }
            Ok(())
        })
        .await?;
    journal
        .event(
            &plan.operation_id,
            "preflight_failed",
            Some(&action.id),
            json!({"error": error, "observed": observed}),
        )
        .await
}

pub(super) async fn fail_restore(
    journal: &Journal,
    plan: &Plan,
    action: &Action,
    error: &str,
) -> Result<(), CmdError> {
    journal
        .update(&plan.operation_id, |state| {
            state.phase = Phase::RestoreFailed;
            state.error = Some(error.to_string());
            if let Some(item) = state.actions.get_mut(&action.id) {
                item.phase = ActionPhase::Failed;
                item.error = Some(error.to_string());
            }
            Ok(())
        })
        .await?;
    journal
        .event(
            &plan.operation_id,
            "restore_failed",
            Some(&action.id),
            json!({"error": error}),
        )
        .await
}

pub(super) async fn fail_action(
    journal: &Journal,
    plan: &Plan,
    action: &Action,
    error: String,
) -> Result<(), CmdError> {
    journal
        .update(&plan.operation_id, |state| {
            state.phase = Phase::ApplyFailed;
            state.error = Some(error.clone());
            if let Some(item) = state.actions.get_mut(&action.id) {
                item.phase = ActionPhase::Failed;
                item.error = Some(error.clone());
            }
            Ok(())
        })
        .await?;
    journal
        .event(
            &plan.operation_id,
            "action_failed",
            Some(&action.id),
            json!({"error": error}),
        )
        .await
}
