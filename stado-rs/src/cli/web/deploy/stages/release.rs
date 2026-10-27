//! Which release is the current one, when the operator did not name a
//! version.

use crate::cli::CmdError;
use crate::release_pipeline::ReleaseRunState;

// Whether a run's bytes are published is the run state's own question:
// `ReleaseRunState::published`. `promoted` is the state `stado release
// submit` leaves a run in once the channel pointer has moved; `reconciled`
// and `completed` are the two states past it. Anything earlier is a build
// in flight, and deploying from one would install bytes whose qualification
// has not been decided.

/// The newest published stable version of one product, from the release run
/// objects `stado release submit` maintains.
///
/// [`crate::cli::release_submit::recent_runs`] is the read side of those
/// objects — the same reader `stado release status` and the operator console
/// print — and it answers newest-first, so the first row that is both
/// `stable` and published is the answer. Nothing here parses a version
/// string to compare it: the run objects are ordered by the store's own write
/// time, and a product whose version numbers went backwards still deployed
/// whatever was promoted last, which is what "current" means.
///
/// The refusal names the product and what was found, because the two ways
/// this fails have different repairs: a product with no run at all has never
/// been submitted, and a product whose newest stable run is still publishing
/// has to finish.
pub(in crate::cli::web::deploy) async fn published_stable_version(
    product: &str,
) -> Result<String, CmdError> {
    // Every run of the product is read: "has no published stable release"
    // is only true of the whole history, not of its newest few.
    let runs = crate::cli::release_submit::recent_runs(Some(product), usize::MAX).await?;
    let Some(newest) = runs.first() else {
        return Err(CmdError::click(format!(
            "no release run has ever been submitted for {product}; run \
             `stado release submit {product} --channel stable` first, or name an exact \
             `--version`"
        ))
        .stating(crate::primitives::failure::FailureCode::NotFound));
    };
    let published = runs.iter().find(|run| {
        run["channel"].as_str() == Some("stable")
            && run["state"]
                .as_str()
                .and_then(ReleaseRunState::named)
                .is_some_and(|state| state.published())
    });
    let Some(run) = published else {
        // The newest run, named so the refusal says what the history holds.
        let field = |name: &str| match newest[name].as_str() {
            Some(value) => value.to_string(),
            None => format!("<the run record has no {name}>"),
        };
        return Err(CmdError::click(format!(
            "{product} has no published stable release; its newest run is {} on the {} channel \
             in state {}. Promote a stable release, or name an exact `--version`",
            field("version"),
            field("channel"),
            field("state"),
        ))
        .stating(crate::primitives::failure::FailureCode::NotFound));
    };
    run["version"]
        .as_str()
        .filter(|version| !version.is_empty())
        .map(str::to_string)
        .ok_or_else(|| {
            CmdError::unreachable(format!(
                "the newest published stable release run for {product} carries no version"
            ))
        })
}
