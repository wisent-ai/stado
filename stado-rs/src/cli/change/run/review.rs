//! The reviewer half of the risk classifier.
//!
//! A review is first the newest round recorded over the candidate's commit
//! with `stado change review` by a session that is not its author: no
//! finding admits it, a finding requests changes. With no such round, it is
//! Tama's harsh review run (`tama reviews harsh run`), one adapter of the
//! review, over a tree staged in an index of its own in the checkout's Git
//! directory, scoped to the candidate's first defect: a reviewer session
//! that is not the author attacks every changed line and stages its repairs
//! in that index. It admits the tree, requests changes, or repairs it; its
//! repairs (of the tree, the message, or both) are kept as a commit, so
//! they are submitted like any other change. A reviewer that ended with no
//! round covering what it left gives no admission either. A machine
//! without Tama runs no review itself: the candidate waits for a recorded
//! round. A high-risk candidate is reviewed before it lands; a low-risk one
//! after, and the repairs of that retroactive review are queued as its
//! follow-up.

use std::path::{Path, PathBuf};

use crate::cli::change::git::{git, ran_with};
use crate::cli::change::record::{Candidate, Review};
use crate::cli::CmdError;
use crate::deploy::host_exec::{installed_program, TAMA_CLI};

/// The verdict of a review whose rounds admit the tree and which repaired
/// nothing.
pub const ADMITTED: &str = "admitted";

/// The verdict of a review whose reviewer repaired the tree or the message.
pub const REPAIRED: &str = "repaired";

/// The verdict of a review that ended with no round covering the tree and
/// message its reviewer left.
pub const NO_ROUND: &str = "no-round";

/// How much of the reviewer's words a review keeps: the end, where Tama
/// prints the rounds' verdict.
const ANSWER_LINES: usize = 40;

/// The wait mechanism's lines on a program's error output: what it waited
/// on, not what it found.
pub const WAIT_LINES: [&str; 3] = ["czekam: ", "koniec czekania: ", "blad czekania: "];

/// A review due `when` over `tree` that did not run, and why.
pub fn not_run(when: &str, tree: &str, answer: String) -> Review {
    Review {
        when: when.to_string(),
        verdict: "not-run".to_string(),
        tree: tree.to_string(),
        answer,
    }
}

/// The commit of a reviewer's repairs on the reviewed commit, and its tree.
pub struct Repair {
    pub commit: String,
    pub tree: String,
}

/// What a reviewer session left behind.
struct Reviewed {
    /// Whether its rounds admit the tree.
    admitted: bool,
    /// Whether a round was recorded at all.
    round: bool,
    /// Its last words.
    answer: String,
    /// The tree its index holds, its repairs included.
    staged: String,
    /// The message as it left it, which it edits when a repair changes
    /// what the message must say.
    message: String,
    /// Whether it rewrote the message: a repair of its own, even over an
    /// unchanged tree.
    rewritten: bool,
}

/// A harsh review, `when` it runs, of `tree`, the tree of `commit`; and the
/// commit of the reviewer's repairs on `commit`, if it made any. `Err` when
/// the reviewer could not be run or its repairs were not committed: no
/// verdict.
pub async fn review(
    candidate: &Candidate,
    when: &str,
    commit: &str,
    tree: &str,
) -> Result<(Review, Option<Repair>), CmdError> {
    if let Some(decided) = recorded(candidate, when, tree)? {
        return Ok((decided, None));
    }
    let Some(defect) = candidate.defects.first() else {
        let why = "the candidate names no defect, and a harsh review is scoped to one; submit it \
                   with --defect";
        return Ok((not_run(when, tree, why.to_string()), None));
    };
    // Tama's harsh review is one adapter of the review: a machine without
    // Tama waits for a round recorded with `stado change review`.
    if let Err(missing) = installed_program(TAMA_CLI) {
        return Err(CmdError::refused(format!(
            "no review round over commit {commit} (tree {tree}) of candidate {id} is recorded by \
             a session that is not its author ({author}), and Tama, whose harsh review would run \
             one, is not installed here ({missing}); a reviewer reads `git -C {checkout} show \
             {commit}` and, from its own session's shell, records `stado change review {id} \
             --findings FILE --improvements FILE`, and the next pass reads it",
            id = candidate.id,
            author = candidate.session.as_deref().unwrap_or("operator"),
            checkout = candidate.checkout.display(),
        )));
    }
    let checkout = &candidate.checkout;
    let area = PathBuf::from(git(checkout, &["rev-parse", "--absolute-git-dir"]).await?)
        .join("stado-change");
    std::fs::create_dir_all(&area)
        .map_err(|error| CmdError::click(format!("cannot create {}: {error}", area.display())))?;
    let index = area.join(format!("{}-{when}.index", candidate.id));
    let message = area.join(format!("{}-{when}.message", candidate.id));
    let author = candidate.session.as_deref();
    let result = reviewed(checkout, &index, &message, (defect, author), commit, tree).await;
    for path in [&index, &message] {
        if let Err(error) = std::fs::remove_file(path) {
            if error.kind() != std::io::ErrorKind::NotFound {
                eprintln!(
                    "stado change run: {} was not removed: {error}",
                    path.display()
                );
            }
        }
    }
    let reviewed = result?;
    let repaired = if reviewed.staged == tree && !reviewed.rewritten {
        None
    } else {
        Some(Repair {
            commit: repairs_commit(checkout, &reviewed, when, commit, tree).await?,
            tree: reviewed.staged.clone(),
        })
    };
    let verdict = if repaired.is_some() {
        REPAIRED
    } else if !reviewed.round {
        NO_ROUND
    } else if reviewed.admitted {
        ADMITTED
    } else {
        "changes-requested"
    };
    Ok((
        Review {
            when: when.to_string(),
            verdict: verdict.to_string(),
            tree: tree.to_string(),
            answer: reviewed.answer,
        },
        repaired,
    ))
}

/// The review the rounds recorded with `stado change review` over `tree`
/// give, `when` it runs: each reviewer's newest round over that tree
/// counts, so a reviewer clears only its own findings and never another's;
/// any finding among them requests changes naming it, and rounds that all
/// found nothing admit the tree. None when no round over `tree` was
/// recorded: a round over another tree (the candidate written again on a
/// `main` that moved, or submitted again) decides nothing, since its
/// reviewer never read this one. A round by the author is never kept, and
/// one by the author's session is left out here as well.
fn recorded(candidate: &Candidate, when: &str, tree: &str) -> Result<Option<Review>, CmdError> {
    let author = candidate.session.as_deref().unwrap_or("operator");
    let mut newest: std::collections::BTreeMap<String, crate::cli::change::record::Round> =
        std::collections::BTreeMap::new();
    for round in crate::cli::change::record::rounds(candidate)? {
        if round.tree != tree || round.reviewer_session == author {
            continue;
        }
        let later = newest
            .get(&round.reviewer_session)
            .is_none_or(|kept| round.at > kept.at);
        if later {
            newest.insert(round.reviewer_session.clone(), round);
        }
    }
    if newest.is_empty() {
        return Ok(None);
    }
    let said = |round: &crate::cli::change::record::Round| {
        round
            .findings
            .iter()
            .map(|finding| match finding {
                serde_json::Value::String(text) => text.clone(),
                other => other.to_string(),
            })
            .collect::<Vec<_>>()
            .join("; ")
    };
    let finding: Vec<String> = newest
        .values()
        .filter(|round| !round.findings.is_empty())
        .map(|round| {
            format!(
                "session {} at {} found {} finding(s): {}",
                round.reviewer_session,
                round.at,
                round.findings.len(),
                said(round)
            )
        })
        .collect();
    let (verdict, answer) = if finding.is_empty() {
        (
            ADMITTED,
            format!(
                "the rounds over tree {tree} by {} found nothing",
                newest
                    .values()
                    .map(|round| format!("session {} at {}", round.reviewer_session, round.at))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        )
    } else {
        (
            "changes-requested",
            format!("the rounds over tree {tree}: {}", finding.join("; ")),
        )
    };
    Ok(Some(Review {
        when: when.to_string(),
        verdict: verdict.to_string(),
        tree: tree.to_string(),
        answer,
    }))
}

/// The reviewer's tree and message committed on `commit`. A repaired tree
/// is noted in the message; a message-only repair is the reviewer's message
/// as it left it.
async fn repairs_commit(
    checkout: &Path,
    reviewed: &Reviewed,
    when: &str,
    commit: &str,
    tree: &str,
) -> Result<String, CmdError> {
    // The commit before the repairs is the reviewed one, so the note names
    // no object; a message a round already noted is not noted again.
    let noted = format!("Repairs the harsh review ({when}) staged over the commit before it.");
    let message = reviewed.message.trim_end();
    let note = if reviewed.staged == tree || message.lines().any(|line| line == noted) {
        format!("{message}\n")
    } else {
        format!("{message}\n\n{noted}\n")
    };
    let made = ran_with(
        checkout,
        &["commit-tree", &reviewed.staged, "-p", commit],
        Some(&note),
        &[],
    )
    .await?;
    if !made.success {
        return Err(CmdError::click(format!(
            "git commit-tree of the reviewer's repairs {} onto {commit} in {} failed: {}",
            reviewed.staged,
            checkout.display(),
            made.stderr.trim()
        )));
    }
    Ok(made.stdout.trim().to_string())
}

/// Run the reviewer over `tree` staged in `index`, with `commit`'s message
/// in `message`, `defect` its scope and `author` the session it is judged
/// as written by.
async fn reviewed(
    checkout: &Path,
    index: &Path,
    message: &Path,
    (defect, author): (&str, Option<&str>),
    commit: &str,
    tree: &str,
) -> Result<Reviewed, CmdError> {
    let env = [("GIT_INDEX_FILE", index.display().to_string())];
    let read = ran_with(checkout, &["read-tree", tree], None, &env).await?;
    if !read.success {
        return Err(CmdError::click(format!(
            "git read-tree {tree} failed: {}",
            read.stderr.trim()
        )));
    }
    let text = git(checkout, &["log", "-1", "--format=%B", commit]).await?;
    std::fs::write(message, &text)
        .map_err(|error| CmdError::click(format!("cannot write {}: {error}", message.display())))?;
    let undeclared =
        super::author::declare(checkout, commit, tree, message, defect, author).await?;
    let shown = format!("tama reviews harsh run --defect {defect} over {tree}");
    let tama = installed_program(TAMA_CLI)
        .map_err(|missing| CmdError::click(format!("tama cannot run: {missing}")))?;
    let mut command = tokio::process::Command::new(tama);
    command
        .args(["reviews", "harsh", "run", "--message"])
        .arg(message)
        .args(["--defect", defect, "--repository"])
        .arg(checkout)
        .current_dir(checkout)
        .env("GIT_INDEX_FILE", index)
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(std::process::Stdio::null())
        .kill_on_drop(true);
    let output = crate::wait::until(
        crate::wait::Kind::Process,
        &shown,
        checkout.display(),
        command.output(),
    )
    .await
    .map_err(|error| CmdError::click(format!("{shown} did not run: {error}")))?;
    let (stdout, stderr) = (
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    // Tama prints the rounds' verdict on its standard output, and exits
    // nonzero both when the rounds do not admit the tree and when no round
    // covers what the reviewer left; only the second leaves its standard
    // output empty and says why on its error output.
    let round = output.status.success() || !stdout.trim().is_empty();
    // The verdict is the end of its standard output; the wait mechanism's
    // lines on its error output say what it waited on, not what it found.
    let lines: Vec<&str> = stderr
        .lines()
        .filter(|line| !WAIT_LINES.iter().any(|wait| line.starts_with(wait)))
        .chain(stdout.lines())
        .filter(|line| !line.trim().is_empty())
        .collect();
    let mut answer = lines[lines.len().saturating_sub(ANSWER_LINES)..].join("\n");
    if let Some(why) = undeclared {
        // Kept first, so the reviewer's last line stays the verdict.
        answer = format!(
            "the candidate's author was not entered, so Tama took the session running the \
             pass as its author: {why}\n{answer}"
        );
    }
    let staged = ran_with(checkout, &["write-tree"], None, &env).await?;
    if !staged.success {
        return Err(CmdError::click(format!(
            "git write-tree of the reviewed index failed: {}",
            staged.stderr.trim()
        )));
    }
    let left = std::fs::read_to_string(message)
        .map_err(|error| CmdError::click(format!("cannot read {}: {error}", message.display())))?;
    Ok(Reviewed {
        admitted: output.status.success(),
        round,
        answer,
        staged: staged.stdout.trim().to_string(),
        rewritten: left.trim_end() != text.trim_end(),
        message: left,
    })
}
