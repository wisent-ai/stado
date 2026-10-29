//! A push to `main`, as the repository's pre-push gate (`stado quality
//! check --pre-push`) judges it beside the `format` gates it runs on this
//! machine.
//!
//! The submit queue's own push is known by the landing its record names for
//! that exact commit ([`landing`]): its pass ran every gate on that commit,
//! so the push is not checked again. Any other push to `main` has the `lint`
//! gates its commit declares run on fleet hosts exactly as the queue runs
//! them ([`lint`]: a Cargo gate as `stado host cargo` runs it), so a commit
//! that does not pass them is refused before it reaches `main` instead of
//! turning `main` red for every session that builds from it. Its `test`
//! gates stay the queue's and the release's.

use std::path::Path;

use super::git;
use super::record::{self, State};
use super::run::gates;
use crate::cli::CmdError;
use crate::release_pipeline::PRODUCT_MANIFEST;

/// Whether a push to `remote_ref` updates a `main` branch. Git's pre-push
/// updates on stdin do not name the remote, so a push of `main` to any
/// remote is taken for the one the queue lands on.
pub(crate) fn updates_main(remote_ref: &str) -> bool {
    remote_ref.strip_prefix("refs/heads/") == Some(git::MAIN)
}

/// The id of the candidate of `checkout`'s repository whose recorded
/// landing is `commit`: a running candidate whose pass is pushing that exact
/// commit, or a landed one that landed it. None when the queue never
/// recorded it.
pub(crate) async fn landing(checkout: &Path, commit: &str) -> Result<Option<String>, CmdError> {
    let repository = git::repository(checkout).await?;
    Ok(record::list(Some(&repository))?
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

/// Run the `lint` gates `commit` of `checkout` declares on the registry
/// `hosts` named, else on those the placement chooses, as the session
/// running this push asks for them (`purposes` named to Oko's admission),
/// and refuse with every refused gate's own report. A commit that holds no
/// release manifest, or declares no `lint` gate, has none to run.
pub(crate) async fn lint(
    checkout: &Path,
    commit: &str,
    (hosts, purposes): (&[String], &[&str]),
) -> Result<(), CmdError> {
    let listed = git::git(
        checkout,
        &["ls-tree", "--name-only", commit, "--", PRODUCT_MANIFEST],
    )
    .await?;
    if listed.is_empty() {
        eprintln!(
            "stado quality check: {commit} holds no {PRODUCT_MANIFEST}, so it declares no lint \
             gate to run before it reaches main"
        );
        return Ok(());
    }
    let plan = gates::plan(checkout, commit, gates::PUSHED)
        .await?
        .map_err(|why| {
            CmdError::refused(format!(
                "the lint gates of {commit} cannot run before it reaches main: {why}"
            ))
        })?;
    if plan.compiling.is_empty() {
        eprintln!(
            "stado quality check: {commit} of {} declares no lint gate to run before it \
             reaches main",
            plan.product
        );
        return Ok(());
    }
    eprintln!(
        "stado quality check: {commit} updates main, so its lint gates run on the fleet: {}",
        plan.names().join(", ")
    );
    let tree = git::git(checkout, &["rev-parse", &format!("{commit}^{{tree}}")]).await?;
    let session = crate::cli::release_loop::allowed::calling_session()
        .await
        .map_err(|cause| {
            CmdError::refused(format!(
                "the lint gates of {commit} are not run: the agent session this push runs under \
                 could not be read ({cause})"
            ))
        })?;
    let (shards, refusal) = gates::compiling(
        checkout,
        commit,
        &tree,
        &plan,
        hosts,
        (purposes, session.as_deref()),
    )
    .await
    .map_err(|error| {
        // A Git hook names no hosts, so a fleet with no host free for the
        // gates is answered with the path that names them: the queue.
        if hosts.is_empty()
            && error.failure == Some(crate::primitives::failure::FailureCode::InfraDown)
        {
            return error.also(format!(
                "a push names no hosts, so land {commit} through the submit queue, whose pass \
                 takes them: `stado change submit {checkout} --commit {commit}`, then `stado \
                 change run {checkout} --host HOST`",
                checkout = checkout.display()
            ));
        }
        error
    })?;
    if let Some(why) = refusal {
        return Err(CmdError::refused(format!(
            "{commit} does not reach main: {why}\nRepair the source the refusal names, commit \
             it and push again"
        )));
    }
    for shard in &shards {
        let how = if shard.cached {
            "answered by the action cache"
        } else {
            "passed"
        };
        eprintln!(
            "stado quality check: {} ({}) for {} on {}: {how}",
            shard.gate, shard.kind, shard.platform, shard.host
        );
    }
    Ok(())
}
