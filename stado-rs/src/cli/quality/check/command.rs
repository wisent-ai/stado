//! Git pre-push protocol inputs select the tree, never checkout dirtiness.
use std::collections::BTreeSet;
use std::io::{self, BufRead};
use std::path::{Path, PathBuf};

use super::{built_revision, export_check, git, Report, Selection};
use crate::cli::CmdError;

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
    let revisions = if pre_push {
        pushed_revisions(&checkout)?
    } else {
        let revision = match revision {
            Some(revision) => resolve(&checkout, revision)?,
            None => built_revision(&checkout)?,
        };
        BTreeSet::from([revision])
    };
    if revisions.is_empty() {
        println!("stado quality check: no source revision is being pushed; nothing to check");
    }
    for revision in revisions {
        println!(
            "stado quality check: checking committed revision {revision} of {}",
            checkout.display()
        );
        export_check(
            &checkout,
            &revision,
            Report::Stdout,
            Selection::Installation,
            if_declared,
        )
        .await
        .map_err(|error| error.within(format!("quality of committed revision {revision}")))?;
    }
    Ok(())
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

fn pushed_revisions(checkout: &Path) -> Result<BTreeSet<String>, CmdError> {
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
            println!("stado quality check: deleting {remote_ref}; no source tree to check");
            continue;
        }
        let revision = resolve(checkout, local_oid)?;
        println!("stado quality check: push {local_ref} -> {remote_ref} selects {revision}");
        revisions.insert(revision);
    }
    Ok(revisions)
}
