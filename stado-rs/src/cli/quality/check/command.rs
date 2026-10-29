//! Git pre-push protocol inputs select the tree, never checkout dirtiness.
use std::collections::BTreeSet;
use std::io::{self, BufRead};
use std::path::{Path, PathBuf};
use std::process::Command;

use super::{export_check, Report, Selection};
use crate::cli::CmdError;

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
/// Oko's admission before a compile or test gate runs.
pub async fn check(
    root: Option<&str>,
    if_declared: bool,
    revision: Option<&str>,
    pre_push: bool,
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
    pre_push: bool,
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
    // Pre-push selects format-kind gates plus source and lock checks. A web
    // format gate can itself run the product's typecheck and lint scripts;
    // the full check selects every gate the installation runs.
    let (revisions, selection) = if pre_push {
        (pushed_revisions(&checkout, report)?, Selection::Formatting)
    } else {
        let revision = match revision {
            Some(revision) => resolve(&checkout, revision)?,
            None => built_revision(&checkout)?,
        };
        (BTreeSet::from([revision]), Selection::Installation)
    };
    if revisions.is_empty() {
        report.say("stado quality check: no source revision is being pushed; nothing to check");
    }
    for revision in &revisions {
        report.say(&format!(
            "stado quality check: checking committed revision {revision} of {}",
            checkout.display()
        ));
        export_check(
            &checkout,
            revision,
            report,
            selection,
            if_declared,
            purposes,
        )
        .await
        .map_err(|error| error.within(format!("quality of committed revision {revision}")))?;
    }
    Ok(revisions)
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

fn pushed_revisions(checkout: &Path, report: Report) -> Result<BTreeSet<String>, CmdError> {
    let mut revisions = BTreeSet::new();
    // Git's pre-push stdin grammar is local-ref local-oid remote-ref remote-oid.
    // A deletion has the null local object ID. These are protocol identities:
    // https://git-scm.com/docs/githooks#_pre_push
    for line in io::stdin().lock().lines() {
        let line = line.map_err(|error| {
            CmdError::click(format!("cannot read Git pre-push updates: {error}"))
        })?;
        let mut fields = line.split_whitespace();
        let (Some(local_ref), Some(local_oid), Some(remote_ref), Some(remote_oid), None) = (
            fields.next(),
            fields.next(),
            fields.next(),
            fields.next(),
            fields.next(),
        ) else {
            return Err(CmdError::refused(format!("invalid Git pre-push update: {line:?}; expected local-ref local-oid remote-ref remote-oid")));
        };
        if !local_oid.bytes().all(|byte| byte.is_ascii_hexdigit())
            || !remote_oid.bytes().all(|byte| byte.is_ascii_hexdigit())
        {
            return Err(CmdError::refused(format!(
                "invalid Git object ID in pre-push update: {line:?}"
            )));
        }
        if local_oid.bytes().all(|byte| byte == b'0') {
            report.say(&format!(
                "stado quality check: deleting {remote_ref}; no source tree to check"
            ));
            continue;
        }
        // A notes ref holds notes about commits, not a source tree; the commits
        // it annotates are checked where their own branch is pushed. Git keeps
        // notes under this namespace: https://git-scm.com/docs/git-notes
        if remote_ref.starts_with("refs/notes/") {
            report.say(&format!(
                "stado quality check: {remote_ref} holds notes, not a source tree; nothing to check"
            ));
            continue;
        }
        let revision = resolve(checkout, local_oid)?;
        report.say(&format!(
            "stado quality check: push {local_ref} -> {remote_ref} selects {revision}"
        ));
        revisions.insert(revision);
    }
    Ok(revisions)
}
