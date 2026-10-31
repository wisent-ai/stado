//! A push to `main`, as the repository's pre-push check (`stado quality
//! check --pre-push`) reads it: the submit queue's own push is known by the
//! landing its record names for that exact commit ([`landing`]); its pass
//! checked that commit, so the push is not checked again. No gate runs at
//! push: any other push has its committed source checked on this machine and
//! nothing formatted, linted or compiled.

use std::path::Path;

use super::git;
use super::record::{self, State};
use crate::cli::CmdError;

/// Whether a push to `remote_ref` updates a `main` branch. Git's pre-push
/// updates on stdin do not name the remote, so a push of `main` to any
/// remote is taken for the one the queue lands on.
pub(crate) fn updates_main(remote_ref: &str) -> bool {
    remote_ref.strip_prefix("refs/heads/") == Some(git::MAIN)
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
