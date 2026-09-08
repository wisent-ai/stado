//! The verify transition: re-inspect every action that reached a stable
//! phase, compare it against the conditions that phase promised, and archive
//! the resulting report beside the operation.

use serde::Serialize;
use serde_json::Value;

use crate::cli::resources::executors::{conditions_match, Context};
use crate::cli::resources::journal::{ActionPhase, Journal, OperationState, Phase};
use crate::cli::resources::model::{Action, Plan};
use crate::cli::resources::VerifyArgs;
use crate::cli::CmdError;

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

pub(in crate::cli::resources::engine) async fn verify_locked(
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

fn conditions_value(conditions: &[super::model::Condition]) -> Value {
    Value::Object(
        conditions
            .iter()
            .map(|condition| (condition.field.clone(), condition.expected.clone()))
            .collect(),
    )
}
