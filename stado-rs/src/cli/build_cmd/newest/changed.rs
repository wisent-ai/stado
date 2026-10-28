//! `stado build newest --most-changed`: the one product the fleet's daily
//! build takes. The operator allows about one build a day for the whole
//! fleet and calls every other build marginal; which product that build is
//! for is decided by the change each one carries — the commits its checkout
//! holds past the newest build of it that passed — never by which session
//! asked first, so a small change asked for early cannot take the day from a
//! larger one.

use std::num::NonZeroUsize;
use std::path::Path;

use crate::cli::release_newest::Planned;
use crate::cli::release_submit::git_text;
use crate::cli::CmdError;
use crate::release_pipeline::BuildRunState;

use super::checkout::buildable;

/// The commits `entry`'s checkout holds at `commit` past the newest build of
/// its product that passed, every commit of its history when none passed;
/// none when it holds nothing new.
async fn change_of(
    root: &Path,
    entry: &Planned,
    commit: &str,
) -> Result<Option<NonZeroUsize>, CmdError> {
    let checkout = root.join(&entry.checkout);
    let builds = crate::cli::build_cmd::report::recent_builds(Some(&entry.product), None).await?;
    let built = builds
        .iter()
        .find(|build| build.state == BuildRunState::Passed);
    let range = match built {
        Some(build) => format!("{}..{commit}", build.source_commit),
        None => commit.to_string(),
    };
    let counted = git_text(&checkout, &["rev-list", "--count", &range])?;
    let count = counted.trim().parse::<usize>().map_err(|error| {
        CmdError::click(format!(
            "git rev-list --count {range} in {} printed {:?}, not a count: {error}",
            checkout.display(),
            counted.trim()
        ))
    })?;
    Ok(NonZeroUsize::new(count))
}

/// The one entry of `planned` that carries the most change since its last
/// passed build, with that count; none when no buildable product carries
/// any. Products are read in name order, so a tie goes to the first name.
pub(super) async fn most_changed(
    root: &Path,
    planned: Vec<Planned>,
) -> Result<Option<(Planned, NonZeroUsize)>, CmdError> {
    let mut chosen: Option<(Planned, NonZeroUsize)> = None;
    for entry in planned {
        let Some((commit, _version)) = buildable(&entry) else {
            continue;
        };
        let Some(change) = change_of(root, &entry, commit).await? else {
            continue;
        };
        let larger = match &chosen {
            Some((_, held)) => change > *held,
            None => true,
        };
        if larger {
            chosen = Some((entry, change));
        }
    }
    Ok(chosen)
}
