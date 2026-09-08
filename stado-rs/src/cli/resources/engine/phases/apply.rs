//! The apply transition: preflight every selected action, then apply the
//! selection in dependency order, re-checking each dependency and each
//! precondition immediately before the mutation it guards.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use crate::cli::resources::engine::report::print_summary;
use crate::cli::resources::engine::selection::validate_selection;
use crate::cli::resources::executors::{conditions_match, explain_mismatch, Context};
use crate::cli::resources::journal::{ActionPhase, Journal, Phase};
use crate::cli::resources::model::{Action, Plan};
use crate::cli::resources::planner;
use crate::cli::CmdError;

use super::failure::{fail_action, fail_preflight};

struct Preflight {
    action_id: String,
    observed: Value,
    already_desired: bool,
    applied_by_operation: bool,
}

pub(in crate::cli::resources::engine) async fn execute_locked(
    journal: &Journal,
    plan: &Plan,
    selected: &BTreeSet<String>,
    irreversible: &[String],
    owner: &str,
    json_output: bool,
) -> Result<(), CmdError> {
    let prior_state = journal.load_state(&plan.operation_id).await?;
    if prior_state.actions.values().any(|item| {
        matches!(
            item.phase,
            ActionPhase::Restoring
                | ActionPhase::Restored
                | ActionPhase::AlreadyRestored
                | ActionPhase::Irreversible
        )
    }) {
        return Err(CmdError::click(
            "operation has entered restore; generate a fresh plan before applying again",
        ));
    }
    let ordered = planner::topological_order(plan)?;
    validate_selection(plan, selected)?;
    let selected_actions: Vec<&Action> = ordered
        .into_iter()
        .filter(|action| selected.contains(&action.id))
        .collect();
    let selected_owned: Vec<Action> = selected_actions
        .iter()
        .map(|action| (**action).clone())
        .collect();
    let context = Context::new(&selected_owned).await?;
    journal
        .update(&plan.operation_id, |state| {
            state.phase = Phase::Preflighting;
            state.error = None;
            Ok(())
        })
        .await?;
    journal
        .event(
            &plan.operation_id,
            "preflight_started",
            None,
            json!({"selected_actions": selected}),
        )
        .await?;

    let mut preflights = Vec::new();
    for action in &selected_actions {
        let observed = match context.inspect(action).await {
            Ok(observed) => observed,
            Err(error) => {
                let message = format!("preflight inspection failed for {}: {error}", action.id);
                fail_preflight(journal, plan, action, None, &message).await?;
                return Err(CmdError::click(message));
            }
        };
        let already_desired = conditions_match(&action.postconditions, &observed);
        if !already_desired && !conditions_match(&action.preconditions, &observed) {
            let detail = explain_mismatch(&action.preconditions, &observed);
            let message = format!("preflight failed for {}: {detail}", action.id);
            fail_preflight(journal, plan, action, Some(&observed), &message).await?;
            return Err(CmdError::click(message));
        }
        let applied_by_operation = prior_state.actions.get(&action.id).is_some_and(|item| {
            matches!(
                item.phase,
                ActionPhase::Applying | ActionPhase::Applied | ActionPhase::Failed
            )
        });
        journal
            .update(&plan.operation_id, |state| {
                if let Some(item) = state.actions.get_mut(&action.id) {
                    if !applied_by_operation {
                        item.phase = ActionPhase::Preflighted;
                    }
                    item.observed_before = Some(observed.clone());
                    item.error = None;
                }
                Ok(())
            })
            .await?;
        preflights.push(Preflight {
            action_id: action.id.clone(),
            observed,
            already_desired,
            applied_by_operation,
        });
    }
    journal.renew(&plan.operation_id, owner).await?;

    journal
        .update(&plan.operation_id, |state| {
            state.phase = Phase::Applying;
            Ok(())
        })
        .await?;
    for action in selected_actions {
        let preflight = preflights
            .iter()
            .find(|preflight| preflight.action_id == action.id)
            .ok_or_else(|| CmdError::click("internal preflight/action mismatch"))?;
        if preflight.already_desired {
            journal
                .update(&plan.operation_id, |state| {
                    if let Some(item) = state.actions.get_mut(&action.id) {
                        item.phase = if preflight.applied_by_operation {
                            ActionPhase::Applied
                        } else {
                            ActionPhase::AlreadyDesired
                        };
                        item.observed_after = Some(preflight.observed.clone());
                    }
                    Ok(())
                })
                .await?;
            continue;
        }
        journal.renew(&plan.operation_id, owner).await?;
        for dependency_id in &action.depends_on {
            let dependency = plan
                .actions
                .iter()
                .find(|candidate| candidate.id == *dependency_id)
                .ok_or_else(|| CmdError::click("validated dependency disappeared"))?;
            let dependency_observed = match context.inspect(dependency).await {
                Ok(observed) => observed,
                Err(error) => {
                    let message = format!(
                        "cannot recheck dependency {} before {}: {error}",
                        dependency.id, action.id
                    );
                    fail_action(journal, plan, action, message.clone()).await?;
                    return Err(CmdError::click(message));
                }
            };
            if !conditions_match(&dependency.postconditions, &dependency_observed) {
                let message = format!(
                    "dependency {} drifted before {}: {}",
                    dependency.id,
                    action.id,
                    explain_mismatch(&dependency.postconditions, &dependency_observed)
                );
                fail_action(journal, plan, action, message.clone()).await?;
                return Err(CmdError::click(message));
            }
        }
        let immediate = match context.inspect(action).await {
            Ok(observed) => observed,
            Err(error) => {
                fail_action(journal, plan, action, error.to_string()).await?;
                return Err(error);
            }
        };
        if conditions_match(&action.postconditions, &immediate) {
            journal
                .update(&plan.operation_id, |state| {
                    if let Some(item) = state.actions.get_mut(&action.id) {
                        item.phase = if preflight.applied_by_operation {
                            ActionPhase::Applied
                        } else {
                            ActionPhase::AlreadyDesired
                        };
                        item.observed_after = Some(immediate.clone());
                    }
                    Ok(())
                })
                .await?;
            continue;
        }
        if !conditions_match(&action.preconditions, &immediate) {
            let message = format!(
                "precondition drifted immediately before {}: {}",
                action.id,
                explain_mismatch(&action.preconditions, &immediate)
            );
            fail_action(journal, plan, action, message.clone()).await?;
            return Err(CmdError::click(message));
        }
        journal
            .update(&plan.operation_id, |state| {
                if let Some(item) = state.actions.get_mut(&action.id) {
                    item.phase = ActionPhase::Applying;
                }
                Ok(())
            })
            .await?;
        journal
            .event(
                &plan.operation_id,
                "action_applying",
                Some(&action.id),
                json!({"resource": action.resource}),
            )
            .await?;
        let receipt = match context.apply(action).await {
            Ok(receipt) => receipt,
            Err(error) => match context.inspect(action).await {
                Ok(observed) if conditions_match(&action.postconditions, &observed) => json!({
                    "reconciled_after_error": error.to_string(),
                    "observed": observed,
                }),
                _ => {
                    fail_action(journal, plan, action, error.to_string()).await?;
                    return Err(error);
                }
            },
        };
        let observed_after = match context.wait_for(action, &action.postconditions).await {
            Ok(observed) => observed,
            Err(error) => match context.inspect(action).await {
                Ok(observed) if conditions_match(&action.postconditions, &observed) => observed,
                _ => {
                    fail_action(journal, plan, action, error.to_string()).await?;
                    return Err(error);
                }
            },
        };
        journal
            .update(&plan.operation_id, |state| {
                if let Some(item) = state.actions.get_mut(&action.id) {
                    item.phase = ActionPhase::Applied;
                    item.observed_after = Some(observed_after.clone());
                    item.receipt = Some(receipt.clone());
                    item.error = None;
                }
                Ok(())
            })
            .await?;
        journal
            .event(
                &plan.operation_id,
                "action_applied",
                Some(&action.id),
                json!({"receipt": receipt, "observed": observed_after}),
            )
            .await?;
    }
    let state = journal
        .update(&plan.operation_id, |state| {
            state.phase = Phase::Applied;
            state.error = None;
            Ok(())
        })
        .await?;
    journal
        .event(&plan.operation_id, "applied", None, json!({}))
        .await?;
    print_summary(plan, &state, selected, irreversible, json_output)
}
