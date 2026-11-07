//! What decides whether one session checkout may go: the checkouts below a
//! root and what git says exists only in one of them; and what went with
//! one that went, for its removal record.

use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::{json, Value};

use super::CLEANER;
use crate::providers::local::space_reclaim::janitor::state::report::{met::Met, CleanupReport};
use crate::providers::local::space_reclaim::weles::dir_size;
use crate::providers::local::space_reclaim::JanitorError;

/// The entry that makes a directory a git checkout: a directory in a
/// repository's own checkout, a file in a linked worktree.
const GIT_ENTRY: &str = ".git";

/// The mode a repository's index records a submodule's commit under.
const SUBMODULE_MODE: &str = "160000";

/// The directories directly below `directory`, sorted. A link is never
/// followed and never listed.
pub(super) fn directories(directory: &Path) -> io::Result<Vec<PathBuf>> {
    let mut found = Vec::new();
    for entry in std::fs::read_dir(directory)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            found.push(entry.path());
        }
    }
    found.sort();
    Ok(found)
}

/// Whether `directory` is a git checkout.
pub(super) fn is_checkout(directory: &Path) -> bool {
    std::fs::symlink_metadata(directory.join(GIT_ENTRY)).is_ok()
}

/// One git answer about `checkout`, or what kept git from answering. The
/// variables that would point git at another repository are cleared, and
/// no optional lock is taken, so reading a checkout never writes its index.
fn git(checkout: &Path, args: &[&str]) -> Result<String, String> {
    let mut command = Command::new("git");
    command
        .arg("--no-optional-locks")
        .arg("-C")
        .arg(checkout)
        .args(args)
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE");
    match crate::wait::output(&mut command) {
        Ok(output) if output.status.success() => {
            Ok(String::from_utf8_lossy(&output.stdout).into_owned())
        }
        Ok(output) => Err(format!(
            "`git {}` in {} answered {} ({})",
            args.join(" "),
            checkout.display(),
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        )),
        Err(error) => Err(format!(
            "`git {}` in {} could not be run: {error}",
            args.join(" "),
            checkout.display()
        )),
    }
}

/// What `checkout` holds that no remote has, in git's words, or `None` when
/// everything in it is on a remote: a change not committed (an untracked
/// file included; what `.gitignore` names is build output and does not
/// count), a repository with no remote at all, a commit that some ref of
/// the checkout reaches (`HEAD`, a branch, a tag, a note, the stash, any
/// tool's ref) and no remote-tracking branch holds, a stash, the same in
/// each submodule checked out in it, and, in a repository's own checkout,
/// another worktree of it that still exists, which taking the repository
/// would break. `Err` when git cannot answer one of these, which keeps the
/// checkout as surely as work would.
pub(super) fn unpushed(checkout: &Path) -> Result<Option<String>, String> {
    let mut held = Vec::new();
    // Asked with the configuration that would hide work overridden: the
    // account's or the checkout's own `status.showUntrackedFiles=no` leaves
    // a new file unlisted, and `submodule.<name>.ignore` or
    // `diff.ignoreSubmodules` a changed submodule, so the checkout would
    // read clean.
    let changes = git(
        checkout,
        &[
            "-c",
            "status.showUntrackedFiles=normal",
            "status",
            "--porcelain",
            "--untracked-files=normal",
            "--ignore-submodules=none",
        ],
    )?;
    if let Some(first) = changes.lines().next() {
        held.push(format!(
            "{} uncommitted or untracked paths, the first `{first}`",
            changes.lines().count()
        ));
    }
    if git(checkout, &["remote"])?.trim().is_empty() {
        held.push("no remote, so its whole history exists only here".to_string());
    } else {
        let commits = git(checkout, &["rev-list", "--all", "--not", "--remotes"])?;
        if let Some(newest) = commits.lines().next() {
            held.push(format!(
                "{} commits no remote-tracking branch holds, the newest {newest}",
                commits.lines().count()
            ));
        }
    }
    let stashes = git(checkout, &["stash", "list"])?;
    if !stashes.trim().is_empty() {
        held.push(format!("{} stashes", stashes.lines().count()));
    }
    if checkout.join(GIT_ENTRY).is_dir() {
        let own = std::fs::canonicalize(checkout)
            .map_err(|error| format!("{} cannot be resolved: {error}", checkout.display()))?;
        let listing = git(checkout, &["worktree", "list", "--porcelain"])?;
        let others: Vec<&str> = listing
            .lines()
            .filter_map(|line| line.strip_prefix("worktree "))
            .filter(|path| Path::new(path) != own.as_path() && Path::new(path).exists())
            .collect();
        if !others.is_empty() {
            held.push(format!(
                "other worktrees of the same repository at {}",
                others.join(", ")
            ));
        }
    }
    for submodule in submodules(checkout)? {
        if let Some(what) = unpushed(&checkout.join(&submodule))? {
            held.push(format!("in its submodule {submodule}: {what}"));
        }
    }
    Ok((!held.is_empty()).then(|| held.join("; ")))
}

/// The submodules checked out in `checkout`, by path: a commit made in one
/// and never pushed is work the checkout's own history does not hold.
fn submodules(checkout: &Path) -> Result<Vec<String>, String> {
    let index = git(checkout, &["ls-files", "--stage", "-z"])?;
    Ok(index
        .split('\0')
        .filter_map(|entry| {
            let (fields, path) = entry.split_once('\t')?;
            let gitlink = fields.split(' ').next() == Some(SUBMODULE_MODE);
            (gitlink && is_checkout(&checkout.join(path))).then(|| path.to_string())
        })
        .collect())
}

/// The bytes at `path`: a directory's whole tree, a file's length.
fn bytes_at(path: &Path) -> i64 {
    match std::fs::symlink_metadata(path) {
        Ok(info) if info.is_dir() => dir_size(path),
        Ok(info) => i64::try_from(info.len()).unwrap_or(i64::MAX),
        Err(_) => i64::default(),
    }
}

/// What `checkout`, about to be taken, holds that no remote has and no keep
/// covers, for its removal record: what `.gitignore` names (build output,
/// but also a `.env` or local state), and the commits only its reflog
/// reaches (work a reset left behind). A wrong removal is then visible in
/// the record. A question git cannot answer is recorded as its cause, so the
/// record never reads as nothing for it.
pub(super) fn went_with(checkout: &Path) -> Value {
    let mut said = Vec::new();
    let mut row = json!({});
    let ignored = [
        "ls-files",
        "-z",
        "--others",
        "--ignored",
        "--exclude-standard",
        "--directory",
    ];
    match git(checkout, &ignored) {
        Ok(listing) => {
            let paths: Vec<&str> = listing
                .split('\0')
                .filter(|path| !path.is_empty())
                .collect();
            let bytes = paths
                .iter()
                .map(|path| bytes_at(&checkout.join(path)))
                .fold(i64::default(), i64::saturating_add);
            if let Some(first) = paths.first() {
                said.push(format!(
                    "{} paths .gitignore names ({bytes} bytes), the first `{first}`",
                    paths.len()
                ));
            }
            row["ignored_paths"] = json!(paths.len());
            row["ignored_bytes"] = json!(bytes);
        }
        Err(cause) => said.push(format!("what .gitignore names was not listed: {cause}")),
    }
    let reflog_only = ["rev-list", "--reflog", "--not", "--remotes", "--all"];
    match git(checkout, &reflog_only) {
        Ok(commits) => {
            if let Some(newest) = commits.lines().next() {
                said.push(format!(
                    "{} commits only its reflog reaches, the newest {newest}",
                    commits.lines().count()
                ));
            }
            row["reflog_only_commits"] = json!(commits.lines().count());
        }
        Err(cause) => said.push(format!(
            "the commits only its reflog reaches were not listed: {cause}"
        )),
    }
    if !said.is_empty() {
        row["said"] = json!(said.join("; "));
    }
    row
}

/// Whether git says `checkout` holds work no remote has, or cannot say.
/// Either keeps it, and the pass's errors say which and what to do.
pub(super) fn holds_work(
    checkout: &Path,
    size: u64,
    met: &mut Met,
    report: &mut CleanupReport,
) -> bool {
    match unpushed(checkout) {
        Ok(None) => false,
        Ok(Some(what)) => {
            met.kept("unpushed_work", size);
            report.add_error(
                CLEANER,
                &JanitorError::os(&format!(
                    "{} is kept: it holds {what}; push that work, and the next pass at the \
                     rule takes the checkout",
                    checkout.display()
                )),
            );
            true
        }
        Err(cause) => {
            met.kept("git_state_unread", size);
            report.add_error(
                CLEANER,
                &JanitorError::os(&format!(
                    "{} is kept: {cause}, so whether it holds work no remote has is not \
                     known; once git answers there, the next pass at the rule judges it again",
                    checkout.display()
                )),
            );
            true
        }
    }
}
