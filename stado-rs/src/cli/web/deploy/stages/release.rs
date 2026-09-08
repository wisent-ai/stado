//! Which release is the current one, when the operator did not name a
//! version.

use crate::cli::CmdError;

/// The release run states that mean the bytes are published and promoted.
///
/// `promoted` is the state `stado release submit` leaves a run in once the
/// channel pointer has moved; `reconciled` and `completed` are the two
/// terminal states past it. Anything earlier — `submitting`, `waiting`,
/// `publishing`, `delivering` — is a build in flight, and deploying from one
/// would install bytes whose qualification has not been decided.
const PUBLISHED_RUN_STATES: [&str; 3] = ["promoted", "reconciled", "completed"];

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
    /// How far back the search looks. A product's own runs are already
    /// filtered by `recent_runs`, so this is a bound on how many of ITS runs
    /// are examined, not on the fleet's history.
    const RUN_WINDOW: usize = 32;

    let runs = crate::cli::release_submit::recent_runs(Some(product), RUN_WINDOW).await?;
    if runs.is_empty() {
        return Err(CmdError::click(format!(
            "no release run has ever been submitted for {product}; run \
             `stado release submit {product} --channel stable` first, or name an exact \
             `--version`"
        )));
    }
    let published = runs.iter().find(|run| {
        run["channel"].as_str() == Some("stable")
            && run["state"]
                .as_str()
                .is_some_and(|state| PUBLISHED_RUN_STATES.contains(&state))
    });
    let Some(run) = published else {
        let newest = runs.first().expect("a non-empty run list has a first row");
        return Err(CmdError::click(format!(
            "{product} has no published stable release; its newest run is {} on the {} channel \
             in state {}. Promote a stable release, or name an exact `--version`",
            newest["version"].as_str().unwrap_or("an unnamed version"),
            newest["channel"].as_str().unwrap_or("unknown"),
            newest["state"].as_str().unwrap_or("unknown"),
        )));
    };
    run["version"]
        .as_str()
        .filter(|version| !version.is_empty())
        .map(str::to_string)
        .ok_or_else(|| {
            CmdError::click(format!(
                "the newest published stable release run for {product} carries no version"
            ))
        })
}
