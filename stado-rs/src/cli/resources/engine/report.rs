//! What an invocation prints: the dry-run preview an operator reviews before
//! approving, and the one summary every finished transition ends on.

use std::collections::BTreeSet;

use serde::Serialize;
use serde_json::json;

use crate::cli::resources::journal::{OperationState, Phase};
use crate::cli::resources::model::Plan;
use crate::cli::CmdError;

#[derive(Debug, Serialize)]
struct ExecutionSummary {
    operation_id: String,
    phase: Phase,
    selected_actions: Vec<String>,
    irreversible_actions: Vec<String>,
    error: Option<String>,
}

pub(super) fn print_preview(
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

pub(super) fn print_summary(
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
