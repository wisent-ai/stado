//! Who authored the tree a harsh review reads. Tama takes the author of a
//! staged tree from Oko's harsh-review queue, and, for a tree no entry
//! names, the session that invokes it: here the session running the pass,
//! which is not who made the change. So before its review the candidate is
//! entered in that queue as staged by the session that submitted it (the
//! operator when none did), and the reviewer's rounds are judged against
//! that author. Oko keeps one open entry per author, so an author whose
//! earlier entry still waits for its round is not entered again; the review
//! runs either way and its record says why the author was not entered.

use std::path::Path;

use crate::cli::change::git::git;
use crate::cli::CmdError;
use crate::deploy::host_exec::{installed_program, OKO_CLI};

/// Enter `tree`, the tree of `commit` in `checkout`, with the message in
/// `message`, in Oko's harsh-review queue as staged by `author` and
/// repairing `defect`: `None` when it was entered, else why it was not.
pub async fn declare(
    checkout: &Path,
    commit: &str,
    tree: &str,
    message: &Path,
    defect: &str,
    author: Option<&str>,
) -> Result<Option<String>, CmdError> {
    let parent = git(checkout, &["rev-parse", &format!("{commit}^")]).await?;
    let author = author.unwrap_or("operator");
    let shown = format!("oko review harsh enqueue --tree {tree} --author {author}");
    let oko = match installed_program(OKO_CLI) {
        Ok(oko) => oko,
        Err(missing) => return Ok(Some(format!("{shown} cannot run: {missing}"))),
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
        Err(error) => return Ok(Some(format!("{shown} did not run: {error}"))),
    };
    if output.status.success() {
        return Ok(None);
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
    Ok(Some(format!(
        "{shown} refused ({}): {}",
        output.status,
        said.join(" ").trim()
    )))
}
