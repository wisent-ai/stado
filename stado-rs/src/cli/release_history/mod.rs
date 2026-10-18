//! `stado release history` — rewrite the commits of one checkout that its
//! upstream does not have yet, by a plan, before they are pushed.
//!
//! A push review refuses a commit that only reformats another, a fixup, a
//! commit that takes back what an earlier one swept in, or a message that
//! says something its patch does not do. Each is repaired by folding,
//! dropping, rewording or splitting unpushed commits, and doing that by hand
//! in a checkout other work shares stops that work. The plan names what
//! becomes of every unpushed commit; the new history is made in a private
//! index inside the repository's git directory, never in the working tree,
//! and the branch moves only when the new history keeps HEAD's tree exactly
//! and the branch still points where it did when the rewrite began. The
//! working tree and the checkout's index are left as they were, since the
//! tree they stand on does not change.

mod plan;
mod replay;

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use clap::Args;
use serde_json::{json, Value};

use crate::cli::CmdError;
use replay::Git;

#[derive(Args)]
pub struct ReleaseHistoryArgs {
    /// The checkout whose unpushed commits are rewritten.
    #[arg(long)]
    root: PathBuf,
    /// The plan: a JSON document `{"commits": [...]}` with one entry per
    /// unpushed commit, in the order the new history keeps them:
    /// `{"action": "pick", "commit": C}`, `{"action": "reword", "commit": C,
    /// "message": FILE}`, `{"action": "fold", "commit": C, "into": K}`,
    /// `{"action": "drop", "commit": C}` or `{"action": "split", "commit": C,
    /// "parts": [{"paths": [...], "message": FILE}, ...]}`.
    #[arg(long)]
    plan: PathBuf,
    /// The notes ref an unaltered commit keeps its note under, such as the
    /// attestation notes a commit standard reads. Without it no note is
    /// carried to the new commits.
    #[arg(long)]
    notes_ref: Option<String>,
    /// Print the outcome as JSON.
    #[arg(long)]
    json: bool,
}

pub fn history(args: &ReleaseHistoryArgs) -> Result<(), CmdError> {
    let probe = Git {
        root: args.root.clone(),
        index: None,
    };
    let git_dir = probe.run(&["rev-parse", "--absolute-git-dir"])?;
    let area = PathBuf::from(git_dir.trim()).join("stado-release-history");
    std::fs::create_dir_all(&area)
        .map_err(|error| CmdError::click(format!("{} cannot be made: {error}", area.display())))?;
    let git = Git {
        root: args.root.clone(),
        index: Some(area.join("index")),
    };
    let outcome = rewrite(&git, args, &area);
    let cleaned = std::fs::remove_dir_all(&area)
        .map_err(|error| CmdError::click(format!("{} cannot be removed: {error}", area.display())));
    let report = outcome?;
    cleaned?;
    if args.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
        return Ok(());
    }
    println!(
        "{} moved from {} to {}",
        report["branch"], report["from"], report["to"]
    );
    for unattested in report["unattested"].as_array().into_iter().flatten() {
        println!("needs its own attestation: {unattested}");
    }
    Ok(())
}

fn rewrite(git: &Git, args: &ReleaseHistoryArgs, area: &Path) -> Result<Value, CmdError> {
    let branch = git.one(&["symbolic-ref", "--short", "HEAD"])?;
    let head = git.one(&["rev-parse", "HEAD"])?;
    let upstream = git.one(&[
        "rev-parse",
        "--abbrev-ref",
        "--symbolic-full-name",
        "@{upstream}",
    ])?;
    let base = git.one(&["merge-base", &upstream, &head])?;
    let range = format!("{base}..{head}");
    let merges = git.one(&["rev-list", "--min-parents=2", &range])?;
    if !merges.is_empty() {
        return Err(CmdError::refused(format!(
            "{range} holds merge commits ({}); a plan rewrites a line of single-parent commits",
            merges.lines().collect::<Vec<_>>().join(", ")
        )));
    }
    let unpushed: Vec<String> = git
        .one(&["rev-list", "--reverse", &range])?
        .lines()
        .map(str::to_string)
        .collect();
    if unpushed.is_empty() {
        return Err(CmdError::refused(format!(
            "{branch} holds no commit {upstream} does not have, so there is nothing to rewrite"
        )));
    }
    let plan = plan::read(&args.plan, &unpushed, |id| {
        git.one(&["rev-parse", "--verify", &format!("{id}^{{commit}}")])
    })?;
    let made = replay::build(git, &base, &plan, area)?;
    let new_head = match made.last() {
        Some(last) => last.commit.clone(),
        None => base.clone(),
    };
    let kept = git.one(&["rev-parse", &format!("{new_head}^{{tree}}")])?;
    let wanted = git.one(&["rev-parse", &format!("{head}^{{tree}}")])?;
    if kept != wanted {
        let differs = git.one(&["diff", "--stat", &head, &new_head])?;
        return Err(CmdError::refused(format!(
            "the plan does not keep HEAD's tree, so {branch} is not moved; it differs \
             in:\n{differs}"
        )));
    }
    let (carried, unattested) = match &args.notes_ref {
        Some(notes_ref) => carry_notes(git, notes_ref, &made)?,
        None => (Vec::new(), Vec::new()),
    };
    // Compare-and-swap: a commit made on the branch since the rewrite began
    // refuses the move instead of being lost.
    git.one(&[
        "update-ref",
        "-m",
        "stado release history",
        &format!("refs/heads/{branch}"),
        &new_head,
        &head,
    ])?;
    Ok(json!({
        "branch": branch,
        "from": head,
        "to": new_head,
        "commits": made.iter().map(|commit| json!({
            "commit": commit.commit,
            "from": commit.from,
            "unaltered": commit.unaltered,
        })).collect::<Vec<_>>(),
        "notes_carried": carried,
        "unattested": unattested,
    }))
}

/// Copy the note of every old commit a new one carries unaltered; the new
/// commits whose note was carried, and those that need their own.
fn carry_notes(
    git: &Git,
    notes_ref: &str,
    made: &[replay::Made],
) -> Result<(Vec<String>, Vec<String>), CmdError> {
    // `git notes list` prints the note object, then the object it annotates.
    let noted: BTreeSet<String> = git
        .one(&["notes", "--ref", notes_ref, "list"])?
        .lines()
        .filter_map(|line| line.split_whitespace().last())
        .map(str::to_string)
        .collect();
    let mut carried = Vec::new();
    let mut unattested = Vec::new();
    for commit in made {
        match commit.from.first() {
            Some(old) if commit.unaltered && noted.contains(old) => {
                git.one(&["notes", "--ref", notes_ref, "copy", old, &commit.commit])?;
                carried.push(commit.commit.clone());
            }
            _ => unattested.push(commit.commit.clone()),
        }
    }
    Ok((carried, unattested))
}
