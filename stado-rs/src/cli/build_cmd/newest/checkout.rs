//! One checkout of `stado build newest`: the commit and version it is built
//! from, how its standing is described, and its build queued.

use crate::cli::build_cmd::{ensure_build, ensure_object_store, read_source, snapshot_source};
use crate::cli::release_newest::{Planned, Standing};
use crate::cli::CmdError;
use crate::release_pipeline::BuildRunState;

/// The commit and version a checkout is built from, whatever its release
/// standing: published or not, its newest commit is what compiles or not.
pub(super) fn buildable(entry: &Planned) -> Option<(&str, &str)> {
    match &entry.standing {
        Standing::Releasable {
            commit, version, ..
        }
        | Standing::Published {
            commit, version, ..
        }
        | Standing::InFlight {
            commit, version, ..
        }
        | Standing::VersionTaken {
            commit, version, ..
        } => Some((commit.as_str(), version.as_str())),
        Standing::DeclaresNoReleases { .. } | Standing::Unreadable { .. } => None,
    }
}

/// What a checkout's standing means for this command, in one sentence.
pub(super) fn describe(entry: &Planned) -> String {
    match &entry.standing {
        Standing::Releasable {
            commit, version, ..
        }
        | Standing::Published {
            commit, version, ..
        }
        | Standing::InFlight {
            commit, version, ..
        }
        | Standing::VersionTaken {
            commit, version, ..
        } => {
            format!("{version} from {commit} would be built")
        }
        Standing::DeclaresNoReleases { reason } => format!("declares no releases: {reason}"),
        Standing::Unreadable { refusal } => format!("cannot be read: {refusal}"),
    }
}

/// One checkout's build, as `stado build submit` would queue it.
pub(super) async fn build_checkout(
    entry: &Planned,
    commit: &str,
    version: &str,
) -> Result<(String, BuildRunState, Option<String>), CmdError> {
    let reading = read_source(&entry.checkout, Some(commit), version)?;
    ensure_object_store().await?;
    let staged = snapshot_source(&reading)?;
    let (build, enqueue_failure) = ensure_build(&reading, &staged, version).await?;
    Ok((
        build.build_id,
        build.state,
        enqueue_failure.map(|error| error.to_string()),
    ))
}
