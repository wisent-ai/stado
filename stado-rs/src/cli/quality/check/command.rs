//! Git pre-push protocol inputs select the tree, never checkout dirtiness.
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;

use super::{export_check, Report, Selection};
use crate::cli::change::git::Remote;
use crate::cli::CmdError;

mod pushed;

/// The remote an install compares a checkout with.
const ORIGIN: &str = "origin";
/// The status `git merge-base --is-ancestor` exits with when the first
/// commit is not an ancestor of the second; any other failure is an error.
const NOT_ANCESTOR: i32 = 1; // https://git-scm.com/docs/git-merge-base ("--is-ancestor ... exit with status 0 if true, or with status 1 if not. Errors are signaled by a non-zero status that is not 1.")

/// The revision an install of `checkout` builds: `origin/main` after a fetch
/// when the tracked tree is clean, the checkout has an `origin` remote and
/// `HEAD` is contained in it, else `HEAD`. A checkout with no `origin` has
/// no main to build from but its own `HEAD`; a fetch from an `origin` that
/// fails, and an ancestry check that answers neither yes nor no, are the
/// error.
pub(super) fn built_revision(checkout: &Path) -> Result<String, CmdError> {
    let head = git(checkout, &["rev-parse", "HEAD"])?;
    let dirty = !git(checkout, &["status", "--porcelain", "--untracked-files=no"])?.is_empty();
    if dirty
        || !git(checkout, &["remote"])?
            .lines()
            .any(|remote| remote == ORIGIN)
    {
        return Ok(head);
    }
    git(checkout, &["fetch", "--quiet", ORIGIN, "main"])?;
    let main = git(checkout, &["rev-parse", "origin/main"])?;
    let ancestry = crate::wait::status(
        Command::new("git")
            .args(["merge-base", "--is-ancestor", "HEAD", "origin/main"])
            .current_dir(checkout),
    )
    .map_err(|error| {
        CmdError::click(format!("cannot run git merge-base: {error}"))
            .stating(crate::cli::entry::error::io_failure_code(error.kind()))
    })?;
    match (ancestry.success(), ancestry.code()) {
        (true, _) => Ok(main),
        (false, Some(NOT_ANCESTOR)) => Ok(head),
        (false, _) => Err(CmdError::click(format!(
            "git merge-base --is-ancestor HEAD origin/main in {} failed ({ancestry}), so whether HEAD is on main is not known",
            checkout.display()
        ))
        .stating(crate::primitives::failure::FailureCode::Config)),
    }
}

fn git(checkout: &Path, args: &[&str]) -> Result<String, CmdError> {
    let output = crate::wait::output(Command::new("git").args(args).current_dir(checkout))
        .map_err(|error| {
            CmdError::click(format!("cannot run git {}: {error}", args.join(" ")))
                .stating(crate::cli::entry::error::io_failure_code(error.kind()))
        })?;
    if !output.status.success() {
        return Err(CmdError::click(format!(
            "git {} in {} failed: {}",
            args.join(" "),
            checkout.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        ))
        .stating(crate::primitives::failure::FailureCode::Config));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

/// Check the selected revisions. With `json`, every progress line and gate
/// report goes to stderr and stdout carries one document: the revisions
/// checked, or the failure envelope whose `details` name the refused gate's
/// findings. `purposes` are the open defects the check serves, named to
/// Oko's admission before a test gate runs or the CLI is built. `pre_push`
/// reads Git's pre-push updates and checks each pushed commit's source,
/// with the remote Git handed the hook when the caller passes it on.
pub async fn check(
    root: Option<&str>,
    if_declared: bool,
    revision: Option<&str>,
    pre_push: Option<Option<&Remote<'_>>>,
    purposes: &[String],
    json: bool,
) -> Result<(), CmdError> {
    let report = if json { Report::Stderr } else { Report::Stdout };
    let purposes: Vec<&str> = purposes.iter().map(String::as_str).collect();
    checked(root, if_declared, revision, pre_push, &purposes, report)
        .await
        .map(|revisions| {
            if json {
                println!(
                    "{}",
                    crate::deploy::host_recovery::to_sorted_pretty(&serde_json::json!({
                        "status": "passed",
                        "revisions": revisions,
                    }))
                );
            }
        })
        .map_err(|error| error.machine_readable(json))
}

async fn checked(
    root: Option<&str>,
    if_declared: bool,
    revision: Option<&str>,
    pre_push: Option<Option<&Remote<'_>>>,
    purposes: &[&str],
    report: Report,
) -> Result<BTreeSet<String>, CmdError> {
    let checkout = match root {
        Some(path) => PathBuf::from(path),
        None => std::env::current_dir().map_err(|error| {
            CmdError::click(format!("cannot read the working directory: {error}"))
                .stating(crate::cli::entry::error::io_failure_code(error.kind()))
        })?,
    };
    if let Some(remote) = pre_push {
        return pushed::pushed(&checkout, if_declared, remote, purposes, report).await;
    }
    let revision = match revision {
        Some(revision) => resolve(&checkout, revision)?,
        None => built_revision(&checkout)?,
    };
    report.say(&format!(
        "stado quality check: checking committed revision {revision} of {}",
        checkout.display()
    ));
    super::instance::refuse_declared(&checkout, &revision).await?;
    export_check(
        &checkout,
        &revision,
        report,
        Selection::Installation,
        if_declared,
        purposes,
    )
    .await
    .map_err(|error| error.within(format!("quality of committed revision {revision}")))?;
    Ok(BTreeSet::from([revision]))
}

/// `stado quality tree`: the one-off commands a model reads in the
/// CLI command tree of `revision` (else the revision an install builds) of
/// the checkout at `root` (else the working directory), built from its
/// committed source as the installation builds it, with no gate run; the
/// build is work the operator's rulings admit for `purposes`, the open
/// defects the reading serves. With
/// `json`, progress goes to stderr and stdout carries one document whose
/// `remaining` lists them; the command exits 0 whenever the tree was judged,
/// empty or not, and non-zero when it could not be built, read or judged.
pub async fn command_tree(
    root: Option<&str>,
    revision: Option<&str>,
    purposes: &[String],
    json: bool,
) -> Result<(), CmdError> {
    let report = if json { Report::Stderr } else { Report::Stdout };
    let checkout = match root {
        Some(path) => PathBuf::from(path),
        None => PathBuf::from("."),
    };
    let revision = match revision {
        Some(revision) => resolve(&checkout, revision),
        None => built_revision(&checkout),
    }
    .map_err(|error| error.machine_readable(json))?;
    let purposes: Vec<&str> = purposes.iter().map(String::as_str).collect();
    export_check(
        &checkout,
        &revision,
        report,
        Selection::CommandTree,
        false,
        &purposes,
    )
    .await
    .map_err(|error| {
        error
            .within(format!("command tree of committed revision {revision}"))
            .machine_readable(json)
    })
}

pub(super) fn resolve(checkout: &Path, revision: &str) -> Result<String, CmdError> {
    git(
        checkout,
        &[
            "rev-parse",
            "--verify",
            "--end-of-options",
            &format!("{revision}^{{commit}}"),
        ],
    )
}
