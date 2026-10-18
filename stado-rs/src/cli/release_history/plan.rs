//! The plan `stado release history` rewrites a checkout's unpushed commits
//! by: one entry per unpushed commit, saying what becomes of it.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::cli::CmdError;

/// What becomes of one unpushed commit.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum Action {
    /// Kept as it is, its message included.
    Pick { commit: String },
    /// Kept, with the message the file holds.
    Reword { commit: String, message: PathBuf },
    /// Its change is applied with the commit `into` names, which keeps its
    /// own message.
    Fold { commit: String, into: String },
    /// Its change is left out. The plan must still keep HEAD's tree, so a
    /// drop is only possible together with what takes the change back.
    Drop { commit: String },
    /// Its change becomes one commit per part, each holding the paths it
    /// names, with the message its file holds.
    Split { commit: String, parts: Vec<Part> },
}

/// One commit a split makes.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Part {
    pub paths: Vec<String>,
    pub message: PathBuf,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct PlanFile {
    commits: Vec<Action>,
}

impl Action {
    pub(super) fn commit(&self) -> &str {
        match self {
            Action::Pick { commit }
            | Action::Reword { commit, .. }
            | Action::Fold { commit, .. }
            | Action::Drop { commit }
            | Action::Split { commit, .. } => commit,
        }
    }
}

/// A plan whose every commit is resolved to its full id.
pub(super) struct Plan {
    /// The entries in the order the new history takes them.
    pub actions: Vec<Action>,
    /// For each commit that receives folds, the folded commits in the order
    /// the old history had them.
    pub folds: BTreeMap<String, Vec<String>>,
}

/// Read the plan at `path` and check it against `unpushed`, the full ids of
/// the commits it rewrites, oldest first: every one appears exactly once, a
/// fold names a commit the new history keeps, and a split names at least one
/// part. `resolve` turns the id a plan writes into the full one.
pub(super) fn read(
    path: &Path,
    unpushed: &[String],
    resolve: impl Fn(&str) -> Result<String, CmdError>,
) -> Result<Plan, CmdError> {
    let text = std::fs::read_to_string(path).map_err(|error| {
        CmdError::usage(format!(
            "the plan {} cannot be read: {error}",
            path.display()
        ))
    })?;
    let file: PlanFile = serde_json::from_str(&text).map_err(|error| {
        CmdError::usage(format!(
            "the plan {} is not a history plan: {error}",
            path.display()
        ))
    })?;
    let mut actions = Vec::new();
    for action in file.commits {
        actions.push(resolved(action, &resolve)?);
    }
    let listed: Vec<&str> = actions.iter().map(Action::commit).collect();
    let unique: BTreeSet<&str> = listed.iter().copied().collect();
    if unique.len() != listed.len() {
        return Err(CmdError::usage(format!(
            "the plan {} names a commit more than once",
            path.display()
        )));
    }
    let expected: BTreeSet<&str> = unpushed.iter().map(String::as_str).collect();
    let missing: Vec<&str> = expected.difference(&unique).copied().collect();
    let foreign: Vec<&str> = unique.difference(&expected).copied().collect();
    if !missing.is_empty() || !foreign.is_empty() {
        return Err(CmdError::usage(format!(
            "the plan {} must name every unpushed commit once and nothing else: it leaves out \
             [{}] and names [{}], which are not unpushed",
            path.display(),
            missing.join(", "),
            foreign.join(", ")
        )));
    }
    let kept: BTreeSet<&str> = actions
        .iter()
        .filter(|action| !matches!(action, Action::Fold { .. } | Action::Drop { .. }))
        .map(Action::commit)
        .collect();
    let mut folds: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for action in &actions {
        match action {
            Action::Fold { commit, into } if !kept.contains(into.as_str()) => {
                return Err(CmdError::usage(format!(
                    "{commit} folds into {into}, which the plan does not keep as a commit"
                )));
            }
            Action::Fold { commit, into } => {
                folds.entry(into.clone()).or_default().push(commit.clone());
            }
            Action::Split { commit, parts } if parts.is_empty() => {
                return Err(CmdError::usage(format!("{commit} is split into no part")));
            }
            _ => {}
        }
    }
    let position: BTreeMap<&str, usize> = unpushed
        .iter()
        .enumerate()
        .map(|(at, commit)| (commit.as_str(), at))
        .collect();
    for folded in folds.values_mut() {
        folded.sort_by_key(|commit| position.get(commit.as_str()).copied());
    }
    Ok(Plan { actions, folds })
}

/// `action` with every commit it names resolved to its full id.
fn resolved(
    action: Action,
    resolve: &impl Fn(&str) -> Result<String, CmdError>,
) -> Result<Action, CmdError> {
    Ok(match action {
        Action::Pick { commit } => Action::Pick {
            commit: resolve(&commit)?,
        },
        Action::Reword { commit, message } => Action::Reword {
            commit: resolve(&commit)?,
            message,
        },
        Action::Fold { commit, into } => Action::Fold {
            commit: resolve(&commit)?,
            into: resolve(&into)?,
        },
        Action::Drop { commit } => Action::Drop {
            commit: resolve(&commit)?,
        },
        Action::Split { commit, parts } => Action::Split {
            commit: resolve(&commit)?,
            parts,
        },
    })
}
