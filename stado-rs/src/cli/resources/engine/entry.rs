//! The command entry points, and the only place the journal lock is taken:
//! each one reads its plan, decides what the invocation is allowed to touch,
//! then hands a locked journal to one transition and releases it afterwards.

use std::collections::BTreeSet;

use crate::cli::resources::journal::Journal;
use crate::cli::resources::model::{Authorization, Intent, Plan, Reversibility};
use crate::cli::resources::planner;
use crate::cli::resources::{ApplyArgs, KillIrrationalArgs, RestoreArgs, VerifyArgs};
use crate::cli::CmdError;

use super::phases::{execute_locked, restore_locked, verify_locked};
use super::report::print_preview;
use super::selection::select_rationalization_actions;

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

pub(crate) async fn execute_autonomous(plan: &Plan) -> Result<(), CmdError> {
    if plan.intent != Intent::AutonomousReconcile {
        return Err(CmdError::click(
            "autonomous executor requires an autonomous_reconcile plan",
        ));
    }
    plan.validate()?;
    if planner::configuration_fingerprint()? != plan.configuration_fingerprint {
        return Err(CmdError::click(
            "Stado configuration changed after autonomous planning; refusing execution",
        ));
    }
    let selected: BTreeSet<String> = plan
        .actions
        .iter()
        .filter(|action| action.authorization == Authorization::Automatic)
        .map(|action| action.id.clone())
        .collect();
    let irreversible = plan
        .actions
        .iter()
        .filter(|action| selected.contains(action.id.as_str()))
        .filter(|action| action.reversibility == Reversibility::Irreversible)
        .map(|action| action.id.clone())
        .collect();
    execute(plan, selected, irreversible, false).await
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
