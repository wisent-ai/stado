//! The pre-push check: Git's ref updates on stdin select the commits it
//! checks, each by its own committed tree, never the checkout's working tree
//! or index, and the remote Git hands the hook decides whether the push is
//! checked at all ([`push::reaches_origin`]).

use std::collections::{BTreeMap, BTreeSet};
use std::io::{self, BufRead};
use std::path::Path;

use super::super::{export_check, Report, Selection};
use super::resolve;
use crate::cli::change::git::Remote;
use crate::cli::change::push;
use crate::cli::CmdError;

/// The pre-push check: every distinct pushed commit gets the source checks
/// (its locks, module declarations and reach) and no gate: nothing at push
/// formats, lints or compiles. A push to another repository than the one
/// `origin` names, which `remote` tells, reaches no shared `main` and is
/// not checked; the updates are still read, so a malformed one refuses. A
/// commit pushed to `main` that the submit queue recorded as its own landing
/// was checked by the queue's pass; it is not checked again and is not among
/// those answered.
pub(super) async fn pushed(
    checkout: &Path,
    if_declared: bool,
    remote: Option<&Remote<'_>>,
    purposes: &[&str],
    report: Report,
) -> Result<BTreeSet<String>, CmdError> {
    let updates = pushed_revisions(checkout, report)?;
    if updates.is_empty() {
        report.say("stado quality check: no source revision is being pushed; nothing to check");
        return Ok(BTreeSet::new());
    }
    if !push::reaches_origin(checkout, remote, report).await? {
        return Ok(BTreeSet::new());
    }
    let mut checked = BTreeSet::new();
    for (revision, main) in &updates {
        if *main {
            if let Some(candidate) = push::landing(checkout, revision).await? {
                report.say(&format!(
                    "stado quality check: {revision} is the submit queue's landing of candidate \
                     {candidate}, whose pass checked it; nothing to check"
                ));
                continue;
            }
        }
        report.say(&format!(
            "stado quality check: checking the committed source of {revision} of {}",
            checkout.display()
        ));
        super::super::instance::refuse_declared(checkout, revision).await?;
        export_check(
            checkout,
            revision,
            report,
            Selection::Source,
            if_declared,
            purposes,
        )
        .await
        .map_err(|error| error.within(format!("quality of committed revision {revision}")))?;
        checked.insert(revision.clone());
    }
    Ok(checked)
}

/// Every distinct commit Git's pre-push updates on stdin push, and whether
/// any update pushing it lands on `main`.
fn pushed_revisions(checkout: &Path, report: Report) -> Result<BTreeMap<String, bool>, CmdError> {
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
        let main = push::updates_main(remote_ref);
        report.say(&format!(
            "stado quality check: push {local_ref} -> {remote_ref} selects {revision}"
        ));
        *revisions.entry(revision).or_insert(false) |= main;
    }
    Ok(revisions)
}
