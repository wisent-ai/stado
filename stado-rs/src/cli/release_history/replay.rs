//! Making the new history in a private index: the checkout's own index,
//! working tree and branch are not touched while the commits are made, so a
//! plan that cannot be carried out leaves the checkout as it was.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;

use super::plan::{Action, Plan};
use crate::cli::CmdError;

/// Git in one checkout; with an index, every index operation is on that
/// private file, never on the checkout's own index.
pub(super) struct Git {
    pub root: PathBuf,
    pub index: Option<PathBuf>,
}

impl Git {
    /// Run `git args` with `env` set; its standard output, or the refusal
    /// naming the command, its exit status and its standard error.
    pub(super) fn run_with(&self, env: &[(&str, &str)], args: &[&str]) -> Result<String, CmdError> {
        let mut command = Command::new("git");
        command.current_dir(&self.root).args(args);
        if let Some(index) = &self.index {
            command.env("GIT_INDEX_FILE", index);
        }
        for (name, value) in env {
            command.env(name, value);
        }
        let shown = format!("git {}", args.join(" "));
        let output = crate::wait::output(&mut command).map_err(|error| {
            CmdError::click(format!(
                "`{shown}` in {} did not run: {error}",
                self.root.display()
            ))
        })?;
        if !output.status.success() {
            return Err(CmdError::refused(format!(
                "`{shown}` in {} exited {}: {}",
                self.root.display(),
                output.status,
                String::from_utf8_lossy(&output.stderr).trim()
            )));
        }
        String::from_utf8(output.stdout).map_err(|error| {
            CmdError::click(format!("`{shown}` printed text that is not UTF-8: {error}"))
        })
    }

    pub(super) fn run(&self, args: &[&str]) -> Result<String, CmdError> {
        self.run_with(&[], args)
    }

    /// The output of `git args` without its surrounding whitespace, for a
    /// command that prints one id, name or line.
    pub(super) fn one(&self, args: &[&str]) -> Result<String, CmdError> {
        Ok(self.run(args)?.trim().to_string())
    }
}

/// One commit of the new history and the old commits its change came from.
pub(super) struct Made {
    pub commit: String,
    pub from: Vec<String>,
    /// Whether it carries the change and message of one old commit
    /// unaltered, so that commit's attestation still describes it.
    pub unaltered: bool,
}

/// One commit an entry of the plan becomes: the paths it is limited to (all
/// when none), its message, and whether it is the old commit unaltered.
struct Piece {
    paths: Vec<String>,
    message: String,
    unaltered: bool,
}

/// Make the commits `plan` asks for on top of `base`, in `area`; the new
/// commits, oldest first.
pub(super) fn build(
    git: &Git,
    base: &str,
    plan: &Plan,
    area: &Path,
) -> Result<Vec<Made>, CmdError> {
    git.run(&["read-tree", base])?;
    let mut parent = base.to_string();
    let mut made = Vec::new();
    for action in &plan.actions {
        let commit = action.commit();
        let folded = match plan.folds.get(commit) {
            Some(folded) => folded.clone(),
            None => Vec::new(),
        };
        let pieces: Vec<Piece> = match action {
            Action::Fold { .. } | Action::Drop { .. } => continue,
            Action::Pick { .. } => vec![Piece {
                paths: Vec::new(),
                message: git.run(&["log", "-1", "--format=%B", commit])?,
                unaltered: folded.is_empty(),
            }],
            Action::Reword { message, .. } => vec![Piece {
                paths: Vec::new(),
                message: read_message(message)?,
                unaltered: false,
            }],
            Action::Split { parts, .. } => {
                covers(git, commit, parts.iter().flat_map(|part| part.paths.iter()))?;
                let mut pieces = Vec::new();
                for part in parts {
                    pieces.push(Piece {
                        paths: part.paths.clone(),
                        message: read_message(&part.message)?,
                        unaltered: false,
                    });
                }
                pieces
            }
        };
        let mut pieces = pieces.into_iter().peekable();
        while let Some(piece) = pieces.next() {
            apply(git, area, commit, &piece.paths)?;
            // Folds join the last commit a kept one becomes.
            if pieces.peek().is_none() {
                for fold in &folded {
                    apply(git, area, fold, &[])?;
                }
            }
            parent = commit_tree(git, area, commit, &parent, &piece.message)?;
            let mut from = vec![commit.to_string()];
            from.extend(folded.iter().cloned());
            made.push(Made {
                commit: parent.clone(),
                from,
                unaltered: piece.unaltered,
            });
        }
    }
    Ok(made)
}

/// The private index as a commit on `parent` with `message`, authored as
/// `commit` was.
fn commit_tree(
    git: &Git,
    area: &Path,
    commit: &str,
    parent: &str,
    message: &str,
) -> Result<String, CmdError> {
    let tree = git.run(&["write-tree"])?.trim().to_string();
    let file = area.join("message");
    std::fs::write(&file, message).map_err(|error| {
        CmdError::click(format!("{} cannot be written: {error}", file.display()))
    })?;
    let author = git.run(&["log", "-1", "--format=%an%n%ae%n%aI", commit])?;
    let mut lines = author.lines();
    let (Some(name), Some(email), Some(date)) = (lines.next(), lines.next(), lines.next()) else {
        return Err(CmdError::refused(format!(
            "git printed no author name, address and date for {commit}"
        )));
    };
    let file_arg = file.to_string_lossy().into_owned();
    let new = git.run_with(
        &[
            ("GIT_AUTHOR_NAME", name),
            ("GIT_AUTHOR_EMAIL", email),
            ("GIT_AUTHOR_DATE", date),
        ],
        &["commit-tree", &tree, "-p", parent, "-F", &file_arg],
    )?;
    Ok(new.trim().to_string())
}

/// Apply the change `commit` made, only under `paths` when any are named,
/// to the private index; refused naming the commit when it does not apply
/// there or holds no change under those paths.
fn apply(git: &Git, area: &Path, commit: &str, paths: &[String]) -> Result<(), CmdError> {
    let parent = format!("{commit}^");
    let mut args = vec![
        "diff-tree",
        "-p",
        "--binary",
        "--full-index",
        "--no-renames",
        parent.as_str(),
        commit,
    ];
    if !paths.is_empty() {
        args.push("--");
        args.extend(paths.iter().map(String::as_str));
    }
    let patch = git.run(&args)?;
    if patch.trim().is_empty() {
        return Err(CmdError::usage(format!(
            "{commit} changes nothing under [{}]",
            paths.join(", ")
        )));
    }
    let file = area.join("patch");
    std::fs::write(&file, &patch).map_err(|error| {
        CmdError::click(format!("{} cannot be written: {error}", file.display()))
    })?;
    git.run(&["apply", "--cached", &file.to_string_lossy()])
        .map(drop)
        .map_err(|cause| {
            CmdError::refused(format!(
                "the change of {commit} does not apply where the plan puts it: {cause}"
            ))
        })
}

/// Refuse a split whose parts do not name every path `commit` changes
/// exactly once.
fn covers<'a>(
    git: &Git,
    commit: &str,
    named: impl Iterator<Item = &'a String>,
) -> Result<(), CmdError> {
    let parent = format!("{commit}^");
    let changed = git.run(&[
        "diff-tree",
        "--name-only",
        "-r",
        "--no-renames",
        "--no-commit-id",
        &parent,
        commit,
    ])?;
    let changed: BTreeSet<&str> = changed.lines().collect();
    let mut seen = BTreeSet::new();
    for path in named {
        if !seen.insert(path.as_str()) {
            return Err(CmdError::usage(format!(
                "{commit} is split with {path} in two parts"
            )));
        }
    }
    let left: Vec<&str> = changed.difference(&seen).copied().collect();
    let foreign: Vec<&str> = seen.difference(&changed).copied().collect();
    if !left.is_empty() || !foreign.is_empty() {
        return Err(CmdError::usage(format!(
            "the parts of {commit} must name every path it changes: they leave out [{}] and \
             name [{}], which it does not change",
            left.join(", "),
            foreign.join(", ")
        )));
    }
    Ok(())
}

fn read_message(path: &Path) -> Result<String, CmdError> {
    let text = std::fs::read_to_string(path).map_err(|error| {
        CmdError::usage(format!(
            "the message {} cannot be read: {error}",
            path.display()
        ))
    })?;
    if text.trim().is_empty() {
        return Err(CmdError::usage(format!(
            "the message {} is empty",
            path.display()
        )));
    }
    Ok(text)
}
