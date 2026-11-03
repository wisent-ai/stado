//! Who authored the tree a harsh review reads. Tama takes the author of a
//! staged tree from Oko's harsh-review queue, and, for a tree no entry
//! names, the session that invokes it: here the session running the pass,
//! which is not who made the change. So before its review the candidate is
//! entered in that queue as staged by the session that submitted it (the
//! operator when none did), and the reviewer's rounds are judged against
//! that author. Oko keeps one open entry per author, so an author whose
//! earlier entry still waits for its round is not entered again: then no
//! review runs, since Tama would judge the tree as written by the session
//! running the pass, and the candidate leaves the head of the queue saying
//! which entry holds its author. A machine without Oko has no queue to enter
//! the author in; there the review runs and its record says so.

use std::path::Path;

use crate::cli::change::git::git;
use crate::cli::CmdError;
use crate::deploy::host_exec::{installed_program, OKO_CLI};

/// Whether the candidate's author was entered before its review.
pub enum Declared {
    /// Entered: Tama reads the author from Oko's queue.
    Entered,
    /// Oko is not installed or did not run, so no queue holds authors; the
    /// review runs with the invoking session as author, and this says why.
    NoQueue(String),
    /// Oko refused the entry (its author already holds an open one); no
    /// review may run, and this says why.
    Refused(String),
}

/// Enter `tree`, the tree of `commit` in `checkout`, with the message in
/// `message`, in Oko's harsh-review queue as staged by `author` and
/// repairing `defect`.
pub async fn declare(
    checkout: &Path,
    commit: &str,
    tree: &str,
    message: &Path,
    defect: &str,
    author: Option<&str>,
) -> Result<Declared, CmdError> {
    let parent = git(checkout, &["rev-parse", &format!("{commit}^")]).await?;
    let author = author.unwrap_or("operator");
    let shown = format!("oko review harsh enqueue --tree {tree} --author {author}");
    let oko = match installed_program(OKO_CLI) {
        Ok(oko) => oko,
        Err(missing) => return Ok(Declared::NoQueue(format!("{shown} cannot run: {missing}"))),
    };
    let mut command = tokio::process::Command::new(oko);
    command
        .args(["review", "harsh", "enqueue", "--repository"])
        .arg(checkout)
        .args(["--parent", &parent, "--tree", tree, "--message-file"])
        .arg(message)
        .args(["--author", author, "--item", defect, "--json"])
        .stdin(std::process::Stdio::null())
        .kill_on_drop(true);
    let output = crate::wait::until(
        crate::wait::Kind::Process,
        &shown,
        checkout.display(),
        command.output(),
    )
    .await;
    let output = match output {
        Ok(output) => output,
        Err(error) => return Ok(Declared::NoQueue(format!("{shown} did not run: {error}"))),
    };
    if output.status.success() {
        return Ok(Declared::Entered);
    }
    // Oko's own words, without the wait mechanism's lines.
    let stderr = String::from_utf8_lossy(&output.stderr);
    let said: Vec<&str> = stderr
        .lines()
        .filter(|line| {
            !super::review::WAIT_LINES
                .iter()
                .any(|wait| line.starts_with(wait))
        })
        .collect();
    Ok(Declared::Refused(format!(
        "{shown} refused ({}): {}",
        output.status,
        said.join(" ").trim()
    )))
}
