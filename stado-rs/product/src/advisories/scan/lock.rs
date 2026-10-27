//! The Cargo lockfiles a checkout's last commit holds, the content each has
//! there, and whether its working file differs from it.
//!
//! A build of the committed source resolves what the committed lock says,
//! so that is what is read: a lock rewritten on disk ships nothing until it
//! is committed, and is named as differing rather than read in place of the
//! committed one. An untracked lock belongs to a build tree or a scratch
//! export and is not read at all.

use crate::common::{capture, checked, files};
use anyhow::{bail, Context, Result};
use std::{
    path::{Path, PathBuf},
    process::Command,
    sync::OnceLock,
};

/// The lockfile name Cargo writes beside a workspace's root manifest.
pub const LOCKFILE: &str = "Cargo.lock";

/// The manifest beside a lockfile that `cargo update` resolves it from.
pub const MANIFEST: &str = "Cargo.toml";

/// The variables git reads as naming one repository, as git lists them.
static LOCAL_VARIABLES: OnceLock<Vec<String>> = OnceLock::new();

/// Clear from `command` every variable git reads as naming one repository
/// (`git rev-parse --local-env-vars`: `GIT_DIR`, `GIT_INDEX_FILE`,
/// `GIT_WORK_TREE` and the rest), so a run from a git hook or with an index
/// of its own still reads, and Cargo still fetches into, the checkout the
/// command names.
pub fn unbound(command: &mut Command) -> Result<&mut Command> {
    if LOCAL_VARIABLES.get().is_none() {
        let output = checked(Command::new("git").args(["rev-parse", "--local-env-vars"]))?;
        let listing = String::from_utf8(output.stdout)
            .context("git rev-parse --local-env-vars printed no UTF-8")?;
        LOCAL_VARIABLES.get_or_init(|| listing.lines().map(str::to_owned).collect());
    }
    for name in LOCAL_VARIABLES.get().into_iter().flatten() {
        command.env_remove(name);
    }
    Ok(command)
}

/// `git -C <checkout> <args>`, reading `checkout` whatever repository the
/// caller's environment names.
fn git(checkout: &Path, args: &[&str]) -> Result<Command> {
    let mut command = Command::new("git");
    command.arg("-C").arg(checkout).args(args);
    unbound(&mut command)?;
    Ok(command)
}

/// What `git -C <checkout> <args>` printed, refused with git's words when
/// it fails.
fn said(checkout: &Path, args: &[&str]) -> Result<String> {
    let output = checked(&mut git(checkout, args)?)?;
    String::from_utf8(output.stdout).with_context(|| {
        format!(
            "git {} in {} printed no UTF-8",
            args.join(" "),
            checkout.display()
        )
    })
}

/// Whether `checkout` has a commit. `git rev-parse --verify --quiet` fails
/// silently for a name that resolves to no commit, as `HEAD` of a checkout
/// that was never committed, and says why on standard error for anything
/// else, as a directory that is no git checkout.
fn committed_head(checkout: &Path) -> Result<bool> {
    let output = capture(&mut git(
        checkout,
        &["rev-parse", "--verify", "--quiet", "HEAD^{commit}"],
    )?)?;
    if output.status.success() {
        return Ok(true);
    }
    let said = String::from_utf8_lossy(&output.stderr);
    if said.trim().is_empty() {
        return Ok(false);
    }
    bail!(
        "git rev-parse HEAD in {} failed ({}): {}",
        checkout.display(),
        output.status,
        said.trim()
    );
}

/// Every lockfile the last commit of `checkout` holds, at any depth, as
/// paths relative to it, in path order; none for a checkout with no commit.
pub fn tracked(checkout: &Path) -> Result<Vec<String>> {
    if !committed_head(checkout)? {
        return Ok(Vec::new());
    }
    let listing = said(
        checkout,
        &["ls-tree", "-r", "-z", "--name-only", "--full-tree", "HEAD"],
    )?;
    let mut files: Vec<String> = listing
        .split('\0')
        .filter(|path| {
            Path::new(path)
                .file_name()
                .is_some_and(|name| name == LOCKFILE)
        })
        .map(str::to_owned)
        .collect();
    files.sort();
    Ok(files)
}

/// The content `lockfile` has in the last commit of `checkout`.
pub fn committed(checkout: &Path, lockfile: &str) -> Result<String> {
    said(checkout, &["cat-file", "blob", &format!("HEAD:{lockfile}")])
}

/// The content of `lockfile`'s working file, or none when the working tree
/// no longer has it.
pub fn working(checkout: &Path, lockfile: &str) -> Result<Option<String>> {
    let path = checkout.join(lockfile);
    if !files::present(&path)? {
        return Ok(None);
    }
    std::fs::read_to_string(&path)
        .map(Some)
        .with_context(|| format!("reading the lockfile {}", path.display()))
}

/// How `lockfile` differs from the last commit of `checkout`, as
/// `git status --porcelain` says it, when it does: rewritten, staged or
/// deleted and not committed.
pub fn changed(checkout: &Path, lockfile: &str) -> Result<Option<String>> {
    let status = said(checkout, &["status", "--porcelain", "--", lockfile])?;
    let status = status.trim();
    Ok(match status.is_empty() {
        true => None,
        false => Some(status.to_owned()),
    })
}

/// The directory a lockfile sits in, where `cargo update` runs against the
/// manifest beside it.
pub fn directory(checkout: &Path, lockfile: &str) -> Result<PathBuf> {
    let path = checkout.join(lockfile);
    path.parent()
        .map(Path::to_path_buf)
        .with_context(|| format!("the lockfile {} names no directory", path.display()))
}
