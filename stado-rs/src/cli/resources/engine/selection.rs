//! Which actions of a plan an invocation is allowed to touch: the closure an
//! operator's approvals expand to, and the check that no selection ever
//! leaves one of its own dependencies behind.

use std::collections::BTreeSet;

use crate::cli::resources::model::{Authorization, Plan};
use crate::cli::CmdError;

pub(super) fn select_rationalization_actions(
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

pub(super) fn validate_selection(plan: &Plan, selected: &BTreeSet<String>) -> Result<(), CmdError> {
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
