//! What decides whether one session checkout may go: the checkouts below a
//! root, what git says exists only in one of them, and whether a launchd or
//! systemd unit on this host names a path in it.

use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

use super::CLEANER;
use crate::deploy::service::{LAUNCHD_UNIT_DIRECTORIES, SYSTEMD_UNIT_DIRECTORIES};
use crate::providers::local::space_reclaim::janitor::state::report::{met::Met, CleanupReport};
use crate::providers::local::space_reclaim::JanitorError;

/// The entry that makes a directory a git checkout: a directory in a
/// repository's own checkout, a file in a linked worktree.
const GIT_ENTRY: &str = ".git";

/// The word the unit directory lists spell the account's home with.
const HOME_WORD: &str = "$HOME/";

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
/// tool's ref) and no remote-tracking branch holds, a stash, and, in a
/// repository's own checkout, another worktree of it that still exists,
/// which taking the repository would break.
pub(super) fn unpushed(checkout: &Path) -> Result<Option<String>, String> {
    let mut held = Vec::new();
    let changes = git(checkout, &["status", "--porcelain"])?;
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
    Ok((!held.is_empty()).then(|| held.join("; ")))
}

/// A launchd or systemd unit definition on this host that names a path in
/// `checkout`: the unit starts that service again even while it is down, so
/// the checkout stays. `Err` names a unit directory or file that could not
/// be read, since absence was then never proven. Links in the unit
/// directories point at definitions listed elsewhere and are not read.
pub(super) fn unit_naming(home: &Path, checkout: &Path) -> Result<Option<PathBuf>, String> {
    let needle = checkout.to_string_lossy();
    for directory in LAUNCHD_UNIT_DIRECTORIES
        .iter()
        .chain(SYSTEMD_UNIT_DIRECTORIES.iter())
    {
        let directory = match directory.strip_prefix(HOME_WORD) {
            Some(relative) => home.join(relative),
            None => PathBuf::from(directory),
        };
        if let Some(unit) = naming_in(&directory, &needle)? {
            return Ok(Some(unit));
        }
    }
    Ok(None)
}

/// The first unit definition at or below `directory` whose text holds
/// `needle`.
fn naming_in(directory: &Path, needle: &str) -> Result<Option<PathBuf>, String> {
    let entries = match std::fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(format!(
                "the unit directory {} cannot be read: {error}",
                directory.display()
            ))
        }
    };
    for entry in entries {
        let entry = entry.map_err(|error| {
            format!(
                "an entry of the unit directory {} cannot be read: {error}",
                directory.display()
            )
        })?;
        let path = entry.path();
        let kind = entry.file_type().map_err(|error| {
            format!("the unit entry {} cannot be read: {error}", path.display())
        })?;
        if kind.is_dir() {
            if let Some(unit) = naming_in(&path, needle)? {
                return Ok(Some(unit));
            }
        } else if kind.is_file() {
            let text = std::fs::read(&path)
                .map_err(|error| format!("the unit {} cannot be read: {error}", path.display()))?;
            if String::from_utf8_lossy(&text).contains(needle) {
                return Ok(Some(path));
            }
        }
    }
    Ok(None)
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
