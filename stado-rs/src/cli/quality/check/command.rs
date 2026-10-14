//! Git pre-push protocol inputs select the tree, never checkout dirtiness.
//!
//! `main` advances only through the delivery queue (`stado ci deliver`): a
//! push names the delivery branch, the queue's one serialized builder runs
//! every installation gate over the pushed commit and moves `main` when they
//! all pass. So a push that names `main` is admitted only for a commit the
//! queue's register records as admitted, and its gates are not run again; a
//! push to any other branch runs the compile-free checks (locks, module
//! declarations, format gates), so no executor builds to push.
use std::collections::BTreeMap;
use std::io::{self, BufRead};
use std::path::{Path, PathBuf};
use std::process::Command;

use super::{export_check, Report, Selection};
use crate::cli::ci::queue::{register, DELIVERY_BRANCH, MAIN};
use crate::cli::CmdError;

/// What a push asks of one revision.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Pushed {
    /// To the delivery branch or any branch but `main`.
    Candidate,
    /// To `main`: only the queue moves it.
    Main,
}

pub async fn check(
    root: Option<&str>,
    if_declared: bool,
    revision: Option<&str>,
    pre_push: bool,
) -> Result<(), CmdError> {
    let checkout = match root {
        Some(path) => PathBuf::from(path),
        None => std::env::current_dir().map_err(|error| {
            CmdError::click(format!("cannot read the working directory: {error}"))
                .stating(crate::cli::entry::error::io_failure_code(error.kind()))
        })?,
    };
    if !pre_push {
        let revision = match revision {
            Some(revision) => resolve(&checkout, revision)?,
            None => built_revision(&checkout)?,
        };
        return checked(&checkout, &revision, Selection::Installation, if_declared).await;
    }
    let revisions = pushed_revisions(&checkout)?;
    if revisions.is_empty() {
        println!("stado quality check: no source revision is being pushed; nothing to check");
    }
    for (revision, pushed) in revisions {
        match pushed {
            Pushed::Main => {
                if !register::admitted(&revision)? {
                    return Err(CmdError::refused(format!(
                        "{revision} of {} is pushed to {MAIN}, and {MAIN} advances only through the \
                         delivery queue: push it to {DELIVERY_BRANCH} (git push origin HEAD:{DELIVERY_BRANCH}); \
                         the queue's builder runs every installation gate over it and moves {MAIN} when \
                         they all pass (`stado ci deliver`, `stado ci status`). The queue register {} \
                         records no admission of it",
                        checkout.display(),
                        register::path()?.display()
                    )));
                }
                println!(
                    "stado quality check: {revision} was admitted by the delivery queue, which ran \
                     every installation gate over it; {MAIN} may move to it"
                );
            }
            // The delivery builder runs the whole installation set; a
            // manifest that declares no compile-free gate leaves the push to it.
            Pushed::Candidate => checked(&checkout, &revision, Selection::Formatting, true).await?,
        }
    }
    Ok(())
}

async fn checked(
    checkout: &Path,
    revision: &str,
    selection: Selection,
    if_declared: bool,
) -> Result<(), CmdError> {
    println!(
        "stado quality check: checking committed revision {revision} of {}",
        checkout.display()
    );
    export_check(checkout, revision, Report::Stdout, selection, if_declared)
        .await
        .map_err(|error| error.within(format!("quality of committed revision {revision}")))
}

fn resolve(checkout: &Path, revision: &str) -> Result<String, CmdError> {
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

/// Every pushed revision and what its push asks: a revision pushed to `main`
/// anywhere in the push is judged as a push to `main`.
fn pushed_revisions(checkout: &Path) -> Result<BTreeMap<String, Pushed>, CmdError> {
    let mut revisions = BTreeMap::new();
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
            println!("stado quality check: deleting {remote_ref}; no source tree to check");
            continue;
        }
        let revision = resolve(checkout, local_oid)?;
        println!("stado quality check: push {local_ref} -> {remote_ref} selects {revision}");
        let pushed = match remote_ref.strip_prefix("refs/heads/") == Some(MAIN) {
            true => Pushed::Main,
            false => Pushed::Candidate,
        };
        let asked = revisions.entry(revision).or_insert(pushed);
        if pushed == Pushed::Main {
            *asked = Pushed::Main;
        }
    }
    Ok(revisions)
}

/// The revision an install of `checkout` builds: `origin/main` after a fetch
/// when the tracked tree is clean and `HEAD` is contained in it, else `HEAD`.
fn built_revision(checkout: &Path) -> Result<String, CmdError> {
    let head = git(checkout, &["rev-parse", "HEAD"])?;
    let dirty = !git(checkout, &["status", "--porcelain", "--untracked-files=no"])?.is_empty();
    if dirty {
        return Ok(head);
    }
    git(checkout, &["fetch", "--quiet", "origin", MAIN])?;
    let main = git(checkout, &["rev-parse", &format!("origin/{MAIN}")])?;
    let contained = crate::wait::status(
        Command::new("git")
            .args([
                "merge-base",
                "--is-ancestor",
                "HEAD",
                &format!("origin/{MAIN}"),
            ])
            .current_dir(checkout),
    )
    .map_err(|error| {
        CmdError::click(format!("cannot run git merge-base: {error}"))
            .stating(crate::cli::entry::error::io_failure_code(error.kind()))
    })?
    .success();
    Ok(if contained { main } else { head })
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
