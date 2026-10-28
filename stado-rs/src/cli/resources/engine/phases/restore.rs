//! The restore transition: walk the applied actions in reverse dependency
//! order, run each rollback whose preconditions still hold, and record the
//! irreversible actions rollback can never reach.

use std::collections::BTreeSet;

use serde_json::json;

use crate::cli::resources::engine::report::print_summary;
use crate::cli::resources::executors::{conditions_match, explain_mismatch, Context};
use crate::cli::resources::journal::{ActionPhase, Journal, OperationState, Phase};
use crate::cli::resources::model::{Action, Plan, Reversibility, Rollback};
use crate::cli::resources::planner;
use crate::cli::CmdError;

use super::failure::{fail_restore, preserve_failure};

pub(in crate::cli::resources::engine) async fn restore_locked(
    journal: &Journal,
    plan: &Plan,
    state: &OperationState,
    owner: &str,
    json_output: bool,
) -> Result<(), CmdError> {
    let ordered = planner::topological_order(plan)?;
    // Each reversible action travels with the rollback and the journalled
    // phase it was selected by, so no later step looks either up again.
    let reversible: Vec<(&Action, &Rollback, ActionPhase)> = ordered
        .into_iter()
        .rev()
        .filter_map(|action| {
            let item = state.actions.get(&action.id)?;
            let rollback = action.rollback.as_ref()?;
            matches!(
                item.phase,
                ActionPhase::Applied | ActionPhase::Restoring | ActionPhase::Failed
            )
            .then_some((action, rollback, item.phase))
        })
        .collect();
    let has_irreversible = state.actions.values().any(|item| {
        item.phase == ActionPhase::Applied
            && plan.actions.iter().any(|action| {
                action.id == item.action_id && action.reversibility == Reversibility::Irreversible
            })
    });
    if reversible.is_empty() && !has_irreversible {
        return Err(CmdError::refused(
            "operation has no applied or indeterminate actions to restore",
        ));
    }
    let reversible_owned: Vec<Action> = reversible
        .iter()
        .map(|(action, _, _)| (*action).clone())
        .collect();
    let context = Context::new(&reversible_owned).await?;
    let irreversible: Vec<String> = state
        .actions
        .values()
        .filter(|item| item.phase == ActionPhase::Applied)
        .filter_map(|item| {
            plan.actions
                .iter()
                .find(|action| action.id == item.action_id)
                .filter(|action| action.reversibility == Reversibility::Irreversible)
                .map(|action| action.id.clone())
        })
        .collect();

    let mut preflight = Vec::new();
    for (action, rollback, prior_phase) in &reversible {
        let observed = context.inspect(action).await?;
        let already_restored = conditions_match(&rollback.postconditions, &observed);
        if !already_restored && !conditions_match(&rollback.preconditions, &observed) {
            return Err(CmdError::refused(format!(
                "restore preflight failed for {}: {}",
                action.id,
                explain_mismatch(&rollback.preconditions, &observed)
            )));
        }
        preflight.push((action.id.as_str(), already_restored, *prior_phase, observed));
    }
    journal.renew(&plan.operation_id, owner).await?;
    journal
        .update(&plan.operation_id, |state| {
            state.phase = Phase::Restoring;
            state.error = None;
            Ok(())
        })
        .await?;
    for (action, rollback, _) in reversible {
        let Some((_, already_restored, prior_phase, observed_before)) =
            preflight.iter().find(|(id, _, _, _)| *id == action.id)
        else {
            return Err(CmdError::click(
                "internal restore preflight/action mismatch",
            ));
        };
        if *already_restored {
            let phase = if *prior_phase == ActionPhase::Restoring
                || (*prior_phase == ActionPhase::Failed && state.phase == Phase::RestoreFailed)
            {
                ActionPhase::Restored
            } else if *prior_phase == ActionPhase::Failed {
                ActionPhase::Skipped
            } else {
                ActionPhase::AlreadyRestored
            };
            journal
                .update(&plan.operation_id, |state| {
                    if let Some(item) = state.actions.get_mut(&action.id) {
                        item.phase = phase;
                        item.observed_after = Some(observed_before.clone());
                    }
                    Ok(())
                })
                .await?;
            continue;
        }
        let receipt = state
            .actions
            .get(&action.id)
            .and_then(|item| item.receipt.as_ref());
        journal.renew(&plan.operation_id, owner).await?;
        let immediate = match context.inspect(action).await {
            Ok(observed) => observed,
            Err(error) => {
                let recorded = fail_restore(journal, plan, action, &error.to_string()).await;
                return Err(preserve_failure(error, recorded, plan, action));
            }
        };
        if conditions_match(&rollback.postconditions, &immediate) {
            let phase = if *prior_phase == ActionPhase::Restoring
                || (*prior_phase == ActionPhase::Failed && state.phase == Phase::RestoreFailed)
            {
                ActionPhase::Restored
            } else if *prior_phase == ActionPhase::Failed {
                ActionPhase::Skipped
            } else {
                ActionPhase::AlreadyRestored
            };
            journal
                .update(&plan.operation_id, |state| {
                    if let Some(item) = state.actions.get_mut(&action.id) {
                        item.phase = phase;
                        item.observed_after = Some(immediate.clone());
                    }
                    Ok(())
                })
                .await?;
            continue;
        }
        if !conditions_match(&rollback.preconditions, &immediate) {
            let message = format!(
                "restore precondition drifted immediately before {}: {}",
                action.id,
                explain_mismatch(&rollback.preconditions, &immediate)
            );
            let recorded = fail_restore(journal, plan, action, &message).await;
            return Err(preserve_failure(
                CmdError::refused(message),
                recorded,
                plan,
                action,
            ));
        }
        journal
            .update(&plan.operation_id, |state| {
                if let Some(item) = state.actions.get_mut(&action.id) {
                    item.phase = ActionPhase::Restoring;
                }
                Ok(())
            })
            .await?;
        let restore_result = match context.restore(action, rollback, receipt).await {
            Ok(_) => {
                context
                    .check_postconditions(action, &rollback.postconditions)
                    .await
            }
            Err(error) => Err(error),
        };
        let restore_result = match restore_result {
            Err(error) => match context.inspect(action).await {
                Ok(observed) if conditions_match(&rollback.postconditions, &observed) => {
                    Ok(observed)
                }
                _ => Err(error),
            },
            result => result,
        };
        let observed = match restore_result {
            Ok(observed) => observed,
            Err(error) => {
                let recorded = fail_restore(journal, plan, action, &error.to_string()).await;
                return Err(preserve_failure(error, recorded, plan, action));
            }
        };
        journal
            .update(&plan.operation_id, |state| {
                if let Some(item) = state.actions.get_mut(&action.id) {
                    item.phase = ActionPhase::Restored;
                    item.observed_after = Some(observed.clone());
                    item.error = None;
                }
                Ok(())
            })
            .await?;
        journal
            .event(
                &plan.operation_id,
                "action_restored",
                Some(&action.id),
                json!({"observed": observed}),
            )
            .await?;
    }
    for action_id in &irreversible {
        journal
            .update(&plan.operation_id, |state| {
                if let Some(item) = state.actions.get_mut(action_id) {
                    item.phase = ActionPhase::Irreversible;
                }
                Ok(())
            })
            .await?;
    }
    let final_state = journal
        .update(&plan.operation_id, |state| {
            state.phase = Phase::Restored;
            state.error = None;
            Ok(())
        })
        .await?;
    journal
        .event(
            &plan.operation_id,
            "restored",
            None,
            json!({"irreversible_actions": irreversible}),
        )
        .await?;
    let selected: BTreeSet<String> = final_state.actions.keys().cloned().collect();
    print_summary(plan, &final_state, &selected, &irreversible, json_output)
}
