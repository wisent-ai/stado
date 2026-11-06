//! A push as the repository's pre-push check (`stado quality check
//! --pre-push`) reads it. Git hands the hook the remote it pushes to, and the
//! check judges only a push that reaches the repository the checkout's
//! `origin` names, the one the submit queue lands on ([`reaches_origin`]): a
//! push to a mirror or a fork carries nothing to the shared `main`, so it is
//! not checked and the check says why. The submit queue's own push is known
//! by the landing its record names for that exact commit ([`landing`]); its
//! pass checked that commit, so the push is not checked again. No gate runs
//! at push: any other push has its committed source checked on this machine
//! and nothing formatted, linted or compiled.

use std::path::Path;

use super::git::{self, Remote, REMOTE};
use super::record::{self, State};
use crate::cli::quality::Report;
use crate::cli::CmdError;

/// Whether a push to `remote_ref` updates a `main` branch; whose `main` it
/// is, [`reaches_origin`] reads from the remote Git hands the hook.
pub(crate) fn updates_main(remote_ref: &str) -> bool {
    remote_ref.strip_prefix("refs/heads/") == Some(git::MAIN)
}

/// Whether a push to `remote` is checked as one reaching the repository
/// `checkout`'s `origin` names. A push Git names as `origin` is, whatever its
/// URL says: one repository has many spellings (an `insteadOf` alias, a
/// forge's second SSH host and port), so a URL that reads otherwise is no
/// reason to skip it. Any other remote is passed, said on `report` with both
/// repositories, only when its URL names a repository none of `origin`'s
/// push URLs names ([`git::named_repository`]; `git remote get-url --push
/// --all origin`): a mirror or a fork. A push is checked, and said so, when
/// that cannot be told: no remote was named (a hook an older Stado wrote, or
/// a dispatcher that does not hand Git's arguments on), the URL names no
/// repository, the checkout has no `origin`, or `origin`'s push URLs name
/// none. Every doubt falls to checking, never to skipping.
pub(crate) async fn reaches_origin(
    checkout: &Path,
    remote: Option<&Remote<'_>>,
    report: Report,
) -> Result<bool, CmdError> {
    let Some(remote) = remote else {
        report.say(&format!(
            "stado quality check: no remote was named (--remote NAME --remote-url URL, Git's two \
             pre-push hook arguments), so this push is taken for one to {REMOTE} and checked; \
             `stado quality hooks install` rewrites a hook an older Stado wrote to name it"
        ));
        return Ok(true);
    };
    if remote.name == REMOTE {
        return Ok(true);
    }
    let Some(pushed) = git::named_repository(remote.url) else {
        report.say(&format!(
            "stado quality check: the URL Git named for {} ({:?}) names no repository, so this \
             push is taken for one to {REMOTE} and checked",
            remote.name, remote.url
        ));
        return Ok(true);
    };
    let remotes = git::git(checkout, &["remote"]).await?;
    if !remotes.lines().any(|name| name == REMOTE) {
        report.say(&format!(
            "stado quality check: {} has no {REMOTE} remote to tell the repository the submit \
             queue lands on, so this push to {} is checked",
            checkout.display(),
            remote.url
        ));
        return Ok(true);
    }
    let urls = git::git(checkout, &["remote", "get-url", "--push", "--all", REMOTE]).await?;
    let named: Vec<String> = urls.lines().filter_map(git::named_repository).collect();
    if named.is_empty() {
        report.say(&format!(
            "stado quality check: {REMOTE}'s push URLs ({urls:?}) name no repository, so this \
             push is taken for one to {REMOTE} and checked"
        ));
        return Ok(true);
    }
    if named.contains(&pushed) {
        return Ok(true);
    }
    report.say(&format!(
        "stado quality check: this push goes to {} ({}), the repository {pushed}, and {REMOTE}, \
         whose {} the submit queue lands on, is {}; nothing it pushes reaches that repository, \
         so it is not checked",
        remote.name,
        remote.url,
        git::MAIN,
        named.join(", ")
    ));
    Ok(false)
}

/// The id of the candidate of `checkout`'s repository whose recorded
/// landing is `commit`: a running candidate whose pass is pushing that exact
/// commit, or a landed one that landed it. None when the queue never
/// recorded it, and none, said on stderr, when this Stado cannot read the
/// queue's records (a newer queue wrote a state this build does not know):
/// the commit's source is then checked, which refuses nothing the check
/// admits, where refusing the read refused every push, the queue's own
/// included.
pub(crate) async fn landing(checkout: &Path, commit: &str) -> Result<Option<String>, CmdError> {
    let repository = git::repository(checkout).await?;
    let candidates = match record::list(Some(&repository)) {
        Ok(candidates) => candidates,
        Err(error) => {
            eprintln!(
                "stado quality check: this Stado cannot read the submit queue's records of \
                 {repository} ({error}), so whether the queue recorded {commit} as its landing \
                 is not known; its source is checked"
            );
            return Ok(None);
        }
    };
    Ok(candidates
        .into_iter()
        .find(|candidate| match candidate.state {
            State::Running => candidate.landing.as_deref() == Some(commit),
            State::Landed => candidate
                .landed
                .as_ref()
                .is_some_and(|landed| landed.commit == commit),
            State::Queued | State::Refused | State::Repaired | State::Cancelled => false,
        })
        .map(|candidate| candidate.id))
}
