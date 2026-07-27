//! Preflight-first, journalled resource plan execution and restore.

use std::collections::BTreeSet;

use serde::Serialize;
use serde_json::{json, Value};

use super::executors::{conditions_match, explain_mismatch, Context};
use super::journal::{ActionPhase, Journal, OperationState, Phase};
use super::model::{Action, Authorization, Intent, Plan, Reversibility};
use super::planner;
use super::{ApplyArgs, CmdError, KillIrrationalArgs, RestoreArgs, VerifyArgs};

#[derive(Debug, Serialize)]
struct ExecutionSummary {
    operation_id: String,
    phase: Phase,
    selected_actions: Vec<String>,
    irreversible_actions: Vec<String>,
    error: Option<String>,
}

#[derive(Debug, Serialize)]
struct VerificationReport {
    schema_version: u8,
    operation_id: String,
    checked_at: String,
    desired_phase: String,
    ok: bool,
    actions: Vec<VerificationAction>,
}

#[derive(Debug, Serialize)]
struct VerificationAction {
    action_id: String,
    resource: String,
    expected: Value,
    observed: Option<Value>,
    ok: bool,
    error: Option<String>,
}

struct Preflight {
    action_id: String,
    observed: Value,
    already_desired: bool,
    applied_by_operation: bool,
}

pub async fn kill_irrational(args: &KillIrrationalArgs) -> Result<(), CmdError> {
    let plan = planner::read_plan(
        &args.plan,
        &args.expect_hash,
        Intent::RationalizationCleanup,
    )?;
    let selected = select_rationalization_actions(&plan, &args.approve)?;
    let irreversible: Vec<String> = plan
        .actions
        .iter()
        .filter(|action| selected.contains(action.id.as_str()))
        .filter(|action| action.reversibility == Reversibility::Irreversible)
        .map(|action| action.id.clone())
        .collect();
    if !args.yes {
        print_preview(&plan, &selected, &irreversible, args.json)?;
        return Ok(());
    }
    let unapproved_irreversible: Vec<&str> = irreversible
        .iter()
        .map(String::as_str)
        .filter(|id| !args.approve.iter().any(|approved| approved == id))
        .collect();
    if !unapproved_irreversible.is_empty() {
        return Err(CmdError::usage(format!(
            "irreversible actions require explicit --approve even when automatic: {}",
            unapproved_irreversible.join(", ")
        )));
    }
    if !irreversible.is_empty() && !args.allow_irreversible {
        return Err(CmdError::usage(format!(
            "selected irreversible actions require --allow-irreversible: {}",
            irreversible.join(", ")
        )));
    }
    execute(&plan, selected, irreversible, args.json).await
}

pub async fn apply_shutdown(args: &ApplyArgs) -> Result<(), CmdError> {
    let plan = planner::read_plan(&args.plan, &args.expect_hash, Intent::Shutdown)?;
    if !args.yes {
        return Err(CmdError::usage(
            "resources apply requires --yes after reviewing the shutdown plan",
        ));
    }
    let selected = plan
        .actions
        .iter()
        .map(|action| action.id.clone())
        .collect();
    execute(&plan, selected, Vec::new(), args.json).await
}

pub async fn verify(args: &VerifyArgs) -> Result<(), CmdError> {
    let journal = Journal::open().await?;
    let plan = journal.load_plan(&args.operation).await?;
    let state = journal.load_state(&args.operation).await?;
    if plan.sha256()? != state.plan_hash {
        return Err(CmdError::click(
            "archived plan hash does not match operation state",
        ));
    }
    let owner = journal.acquire(&plan.operation_id).await?;
    let result = verify_locked(&journal, &plan, &state, &owner, args).await;
    let release = journal.release(&plan.operation_id, &owner).await;
    match (result, release) {
        (Err(error), _) => Err(error),
        (Ok(()), Err(error)) => Err(error),
        (Ok(()), Ok(())) => Ok(()),
    }
}

async fn verify_locked(
    journal: &Journal,
    plan: &Plan,
    state: &OperationState,
    owner: &str,
    args: &VerifyArgs,
) -> Result<(), CmdError> {
    if matches!(
        state.phase,
        Phase::Planned | Phase::Preflighting | Phase::Applying | Phase::Restoring
    ) {
        return Err(CmdError::click(format!(
            "operation {} is {:?}; there is no stable applied/restored state to verify",
            plan.operation_id, state.phase
        )));
    }
    let restored = state.actions.values().any(|item| {
        matches!(
            item.phase,
            ActionPhase::Restored | ActionPhase::AlreadyRestored | ActionPhase::Irreversible
        )
    });
    let inspectable: Vec<Action> = plan
        .actions
        .iter()
        .filter(|action| {
            state.actions.get(&action.id).is_some_and(|item| {
                matches!(
                    item.phase,
                    ActionPhase::Applied
                        | ActionPhase::AlreadyDesired
                        | ActionPhase::Restored
                        | ActionPhase::AlreadyRestored
                        | ActionPhase::Irreversible
                )
            })
        })
        .cloned()
        .collect();
    let context = Context::new(&inspectable).await?;
    journal.renew(&plan.operation_id, owner).await?;
    journal
        .update(&plan.operation_id, |state| {
            state.phase = Phase::Verifying;
            state.error = None;
            Ok(())
        })
        .await?;
    let mut actions = Vec::new();
    for action in &plan.actions {
        let Some(action_state) = state.actions.get(&action.id) else {
            continue;
        };
        if matches!(
            action_state.phase,
            ActionPhase::Pending | ActionPhase::Preflighted | ActionPhase::Skipped
        ) {
            continue;
        }
        if matches!(
            action_state.phase,
            ActionPhase::Applying | ActionPhase::Failed | ActionPhase::Restoring
        ) {
            actions.push(VerificationAction {
                action_id: action.id.clone(),
                resource: action.resource.reference.clone(),
                expected: Value::Null,
                observed: None,
                ok: false,
                error: Some(format!(
                    "action has indeterminate journal phase {:?}",
                    action_state.phase
                )),
            });
            continue;
        }
        journal.renew(&plan.operation_id, owner).await?;
        let use_rollback = restored
            && matches!(
                action_state.phase,
                ActionPhase::Restored | ActionPhase::AlreadyRestored
            );
        let expected_conditions = if use_rollback {
            action
                .rollback
                .as_ref()
                .map(|rollback| rollback.postconditions.as_slice())
                .unwrap_or(action.postconditions.as_slice())
        } else {
            action.postconditions.as_slice()
        };
        match context.inspect(action).await {
            Ok(observed) => actions.push(VerificationAction {
                action_id: action.id.clone(),
                resource: action.resource.reference.clone(),
                expected: conditions_value(expected_conditions),
                ok: conditions_match(expected_conditions, &observed),
                observed: Some(observed),
                error: None,
            }),
            Err(error) => actions.push(VerificationAction {
                action_id: action.id.clone(),
                resource: action.resource.reference.clone(),
                expected: conditions_value(expected_conditions),
                observed: None,
                ok: false,
                error: Some(error.to_string()),
            }),
        }
    }
    let ok = actions.iter().all(|action| action.ok);
    let report = VerificationReport {
        schema_version: super::model::SCHEMA_VERSION,
        operation_id: plan.operation_id.clone(),
        checked_at: chrono::Utc::now().to_rfc3339(),
        desired_phase: if restored { "restored" } else { "applied" }.to_string(),
        ok,
        actions,
    };
    journal
        .write_artifact(&plan.operation_id, "verification-latest.json", &report)
        .await?;
    journal
        .update(&plan.operation_id, |state| {
            state.phase = if ok { Phase::Verified } else { Phase::Drifted };
            state.error = (!ok).then(|| "verification found drift".to_string());
            Ok(())
        })
        .await?;
    journal
        .event(
            &plan.operation_id,
            if ok { "verified" } else { "drifted" },
            None,
            serde_json::to_value(&report)?,
        )
        .await?;
    if args.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!(
            "operation {} verification: {}; {} action(s)",
            plan.operation_id,
            if ok { "ok" } else { "drifted" },
            report.actions.len()
        );
        for action in &report.actions {
            if !action.ok {
                println!(
                    "DRIFT\t{}\t{}\t{}",
                    action.action_id,
                    action.resource,
                    action.error.as_deref().unwrap_or("postcondition mismatch")
                );
            }
        }
    }
    if !ok {
        return Err(CmdError::silent(true as i32));
    }
    Ok(())
}

pub async fn restore(args: &RestoreArgs) -> Result<(), CmdError> {
    if !args.yes {
        return Err(CmdError::usage(
            "resources restore requires --yes after reviewing rollback coverage",
        ));
    }
    let journal = Journal::open().await?;
    let plan = journal.load_plan(&args.operation).await?;
    let state = journal.load_state(&args.operation).await?;
    if plan.sha256()? != state.plan_hash {
        return Err(CmdError::click(
            "archived plan hash does not match operation state",
        ));
    }
    let owner = journal.acquire(&plan.operation_id).await?;
    let result = restore_locked(&journal, &plan, &state, &owner, args.json).await;
    let release = journal.release(&plan.operation_id, &owner).await;
    match (result, release) {
        (Err(error), _) => Err(error),
        (Ok(()), Err(error)) => Err(error),
        (Ok(()), Ok(())) => Ok(()),
    }
}

async fn execute(
    plan: &Plan,
    selected: BTreeSet<String>,
    irreversible: Vec<String>,
    json_output: bool,
) -> Result<(), CmdError> {
    let journal = Journal::open().await?;
    journal.create(plan).await?;
    let owner = journal.acquire(&plan.operation_id).await?;
    let result = execute_locked(
        &journal,
        plan,
        &selected,
        &irreversible,
        &owner,
        json_output,
    )
    .await;
    let release = journal.release(&plan.operation_id, &owner).await;
    match (result, release) {
        (Err(error), _) => Err(error),
        (Ok(()), Err(error)) => Err(error),
        (Ok(()), Ok(())) => Ok(()),
    }
}

async fn execute_locked(
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

async fn restore_locked(
    journal: &Journal,
    plan: &Plan,
    state: &OperationState,
    owner: &str,
    json_output: bool,
) -> Result<(), CmdError> {
    let ordered = planner::topological_order(plan)?;
    let reversible: Vec<&Action> = ordered
        .into_iter()
        .rev()
        .filter(|action| {
            state.actions.get(&action.id).is_some_and(|item| {
                matches!(
                    item.phase,
                    ActionPhase::Applied | ActionPhase::Restoring | ActionPhase::Failed
                )
            }) && action.rollback.is_some()
        })
        .collect();
    let has_irreversible = state.actions.values().any(|item| {
        item.phase == ActionPhase::Applied
            && plan.actions.iter().any(|action| {
                action.id == item.action_id && action.reversibility == Reversibility::Irreversible
            })
    });
    if reversible.is_empty() && !has_irreversible {
        return Err(CmdError::click(
            "operation has no applied or indeterminate actions to restore",
        ));
    }
    let reversible_owned: Vec<Action> =
        reversible.iter().map(|action| (**action).clone()).collect();
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
    for action in &reversible {
        let rollback = action.rollback.as_ref().expect("filtered rollback");
        let observed = context.inspect(action).await?;
        let already_restored = conditions_match(&rollback.postconditions, &observed);
        if !already_restored && !conditions_match(&rollback.preconditions, &observed) {
            return Err(CmdError::click(format!(
                "restore preflight failed for {}: {}",
                action.id,
                explain_mismatch(&rollback.preconditions, &observed)
            )));
        }
        let prior_phase = state.actions[&action.id].phase;
        preflight.push((action.id.as_str(), already_restored, prior_phase, observed));
    }
    journal.renew(&plan.operation_id, owner).await?;
    journal
        .update(&plan.operation_id, |state| {
            state.phase = Phase::Restoring;
            state.error = None;
            Ok(())
        })
        .await?;
    for action in reversible {
        let rollback = action.rollback.as_ref().expect("filtered rollback");
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
                fail_restore(journal, plan, action, &error.to_string()).await?;
                return Err(error);
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
            fail_restore(journal, plan, action, &message).await?;
            return Err(CmdError::click(message));
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
            Ok(_) => context.wait_for(action, &rollback.postconditions).await,
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
                fail_restore(journal, plan, action, &error.to_string()).await?;
                return Err(error);
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

async fn fail_preflight(
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

async fn fail_restore(
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

async fn fail_action(
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

fn select_rationalization_actions(
    plan: &Plan,
    approved: &[String],
) -> Result<BTreeSet<String>, CmdError> {
    let known: BTreeSet<&str> = plan
        .actions
        .iter()
        .map(|action| action.id.as_str())
        .collect();
    for id in approved {
        if !known.contains(id.as_str()) {
            return Err(CmdError::usage(format!(
                "--approve references unknown action {id:?}"
            )));
        }
    }
    let mut selected: BTreeSet<String> = plan
        .actions
        .iter()
        .filter(|action| action.authorization == Authorization::Automatic)
        .map(|action| action.id.clone())
        .chain(approved.iter().cloned())
        .collect();
    loop {
        let before = selected.len();
        let dependencies: Vec<String> = plan
            .actions
            .iter()
            .filter(|action| selected.contains(&action.id))
            .flat_map(|action| action.depends_on.iter().cloned())
            .collect();
        selected.extend(dependencies);
        if selected.len() == before {
            break;
        }
    }
    Ok(selected)
}

fn validate_selection(plan: &Plan, selected: &BTreeSet<String>) -> Result<(), CmdError> {
    for action in plan
        .actions
        .iter()
        .filter(|action| selected.contains(&action.id))
    {
        if action
            .depends_on
            .iter()
            .any(|dependency| !selected.contains(dependency))
        {
            return Err(CmdError::click(format!(
                "selected action {} is missing a dependency",
                action.id
            )));
        }
    }
    Ok(())
}

fn print_preview(
    plan: &Plan,
    selected: &BTreeSet<String>,
    irreversible: &[String],
    json_output: bool,
) -> Result<(), CmdError> {
    if json_output {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "operation_id": plan.operation_id,
                "dry_run": true,
                "selected_actions": selected,
                "irreversible_actions": irreversible,
            }))?
        );
    } else {
        println!(
            "Previewing {} action(s) for operation {}.",
            selected.len(),
            plan.operation_id
        );
        for action in plan
            .actions
            .iter()
            .filter(|action| selected.contains(&action.id))
        {
            println!(
                "{}\t{:?}\t{}\t{:?}",
                action.id, action.kind, action.resource.reference, action.reversibility
            );
        }
        if !irreversible.is_empty() {
            println!("IRREVERSIBLE: {}", irreversible.join(", "));
        }
    }
    Ok(())
}

fn print_summary(
    plan: &Plan,
    state: &OperationState,
    selected: &BTreeSet<String>,
    irreversible: &[String],
    json_output: bool,
) -> Result<(), CmdError> {
    let summary = ExecutionSummary {
        operation_id: plan.operation_id.clone(),
        phase: state.phase,
        selected_actions: selected.iter().cloned().collect(),
        irreversible_actions: irreversible.to_vec(),
        error: state.error.clone(),
    };
    if json_output {
        println!("{}", serde_json::to_string_pretty(&summary)?);
    } else {
        println!(
            "operation {}: {:?}; {} selected action(s); {} irreversible action(s)",
            summary.operation_id,
            summary.phase,
            summary.selected_actions.len(),
            summary.irreversible_actions.len()
        );
    }
    Ok(())
}

fn conditions_value(conditions: &[super::model::Condition]) -> Value {
    Value::Object(
        conditions
            .iter()
            .map(|condition| (condition.field.clone(), condition.expected.clone()))
            .collect(),
    )
}
