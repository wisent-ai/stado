//! The candidate's own commits written again on the remote tip, one by one,
//! with no working tree and no index: each commit's change is merged onto
//! the commit before it by `git merge-tree --write-tree` (its parent as the
//! merge base), and the merged tree is committed with the commit's own
//! author, date and message. A commit whose change the tip already holds
//! adds nothing and is left out; a commit Git cannot merge refuses the
//! candidate, naming the files, and so do a merge commit among its own and
//! a candidate that shares no history with the tip.

use std::path::Path;

use crate::cli::change::git::{contains, git, ran, ran_with};
use crate::cli::change::record::Rebased;
use crate::cli::CmdError;

/// How writing the candidate on the tip ended.
pub enum Rewritten {
    Done(Rebased),
    /// The tip already holds the candidate's commit, or the change of each
    /// of its own commits.
    AlreadyOnMain,
    /// The candidate cannot be written on the tip (a commit Git cannot
    /// merge, a merge commit, no shared history), in the sentence the
    /// candidate keeps.
    Conflict(String),
}

/// Write `commit`'s own commits (those past its merge base with `tip`) on
/// `tip`.
pub async fn rebase(checkout: &Path, commit: &str, tip: &str) -> Result<Rewritten, CmdError> {
    if contains(checkout, commit, tip).await? {
        return Ok(Rewritten::AlreadyOnMain);
    }
    // `git merge-base` exits 1 and says nothing when the two share no
    // history: the candidate's refusal, never a Git that failed.
    let based = ran(checkout, &["merge-base", commit, tip], None).await?;
    let base = based.stdout.trim().to_string();
    if !based.success {
        if base.is_empty() && based.stderr.trim().is_empty() {
            return Ok(Rewritten::Conflict(format!(
                "{commit} shares no history with main {tip}, so it holds no change of its own \
                 to write on it; commit the change on main and submit that commit"
            )));
        }
        return Err(CmdError::click(format!(
            "git merge-base {commit} {tip} in {} failed: {}",
            checkout.display(),
            based.stderr.trim()
        )));
    }
    let range = format!("{base}..{commit}");
    let merges = git(checkout, &["rev-list", "--merges", &range]).await?;
    if !merges.is_empty() {
        return Ok(Rewritten::Conflict(format!(
            "{commit} carries merge commits past its merge base {base} with main ({}); the \
             queue writes linear commits on the tip, so submit the change as linear commits",
            merges.lines().collect::<Vec<_>>().join(", ")
        )));
    }
    let listed = git(checkout, &["rev-list", "--reverse", &range]).await?;
    let own: Vec<&str> = listed.lines().filter(|line| !line.is_empty()).collect();
    let mut parent = tip.to_string();
    for each in &own {
        let merge_base = format!("--merge-base={each}^");
        let merged = ran(
            checkout,
            &[
                "merge-tree",
                "--write-tree",
                "--name-only",
                "--no-messages",
                &merge_base,
                &parent,
                each,
            ],
            None,
        )
        .await?;
        let mut lines = merged.stdout.lines();
        let tree = lines.next().map_or("", str::trim).to_string();
        let is_tree = tree.len() >= 40 && tree.chars().all(|c| c.is_ascii_hexdigit());
        if !merged.success {
            if !is_tree {
                return Err(CmdError::click(format!(
                    "git merge-tree of {each} onto {parent} in {} failed: {}",
                    checkout.display(),
                    merged.stderr.trim()
                )));
            }
            let files: Vec<&str> = lines.take_while(|line| !line.is_empty()).collect();
            return Ok(Rewritten::Conflict(format!(
                "{each} of the candidate does not merge onto main {tip}: Git cannot merge {}; \
                 rebase the change onto the current main and submit the new commit",
                files.join(", ")
            )));
        }
        let parent_tree = git(checkout, &["rev-parse", &format!("{parent}^{{tree}}")]).await?;
        if tree == parent_tree {
            continue;
        }
        parent = recommit(checkout, each, &tree, &parent).await?;
    }
    if parent == tip {
        return Ok(Rewritten::AlreadyOnMain);
    }
    let tree = git(checkout, &["rev-parse", &format!("{parent}^{{tree}}")]).await?;
    Ok(Rewritten::Done(Rebased {
        base: tip.to_string(),
        commit: parent,
        tree,
        commits: own.len(),
    }))
}

/// `tree` committed on `parent` as `original` was: its author, author date
/// and message.
async fn recommit(
    checkout: &Path,
    original: &str,
    tree: &str,
    parent: &str,
) -> Result<String, CmdError> {
    let author = git(
        checkout,
        &[
            "log",
            "-1",
            "--format=%an%x00%ae%x00%ad",
            "--date=raw",
            original,
        ],
    )
    .await?;
    let mut fields = author.split('\0');
    let (Some(name), Some(email), Some(date)) = (fields.next(), fields.next(), fields.next())
    else {
        return Err(CmdError::click(format!(
            "git log of {original} named no author, email and date: {author}"
        )));
    };
    let message = ran(checkout, &["log", "-1", "--format=%B", original], None).await?;
    if !message.success {
        return Err(CmdError::click(format!(
            "the message of {original} cannot be read: {}",
            message.stderr.trim()
        )));
    }
    let env = [
        ("GIT_AUTHOR_NAME", name.to_string()),
        ("GIT_AUTHOR_EMAIL", email.to_string()),
        ("GIT_AUTHOR_DATE", date.to_string()),
    ];
    let written = ran_with(
        checkout,
        &["commit-tree", tree, "-p", parent],
        Some(&format!("{}\n", message.stdout.trim_end())),
        &env,
    )
    .await?;
    if !written.success {
        return Err(CmdError::click(format!(
            "git commit-tree of {original}'s change onto {parent} in {} failed: {}",
            checkout.display(),
            written.stderr.trim()
        )));
    }
    Ok(written.stdout.trim().to_string())
}
