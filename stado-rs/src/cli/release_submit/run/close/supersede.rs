//! One product, one channel, one run worth building at a time.
//!
//! Ten agents submitting every few seconds used to queue ten runs and twenty
//! builds, and every one of them was built, signed and published; only the
//! delivery fence (`latest_submitted_run`) kept the older ones off the hosts.
//! Here a submission of the same product and channel supersedes the live runs
//! it replaces ([`replaces`]: a lower version, or the same version submitted
//! earlier): their builds still waiting in the queue are cancelled, and a
//! build already running is left to end but is not published, because by
//! then a run that replaces it exists. The run says which one replaced it.

use crate::cli::release_submit::run::state::save;
use crate::cli::work::cancel::cancel_queued_in_store;
use crate::cli::CmdError;
use crate::queue::storage::JobStorage;
use crate::release_pipeline::{PlatformRunState, ReleaseRun, ReleaseRunState};

/// Runs of the same product and channel that were created before `newest`
/// and have not ended. `Failed` and `Superseded` runs are already over;
/// `Completed`, `Promoted` and `Reconciled` runs have published and stay.
async fn live_runs_of(
    store: &JobStorage,
    product: &str,
    channel: crate::release_pipeline::PipelineChannel,
) -> Result<Vec<ReleaseRun>, CmdError> {
    let mut runs = Vec::new();
    for path in store
        .list_paths("runs/release-pipeline/", 0)
        .await
        .map_err(|error| CmdError::click(error.to_string()))?
    {
        if !path.ends_with("/run.json") {
            continue;
        }
        let Some(text) = store
            .download_text(&path)
            .await
            .map_err(|error| CmdError::click(error.to_string()))?
        else {
            continue;
        };
        // A run record this build cannot read whole - written by another
        // version, or seeded partially - is not one this submission may
        // supersede; it is left alone and named, never a reason to refuse
        // the submission.
        let run: ReleaseRun = match serde_json::from_str(&text) {
            Ok(run) => run,
            Err(error) => {
                eprintln!("release run {path} left alone: {error}");
                continue;
            }
        };
        if run.product == product
            && run.channel == channel
            && matches!(
                run.state,
                ReleaseRunState::Submitting
                    | ReleaseRunState::Waiting
                    | ReleaseRunState::Publishing
            )
        {
            runs.push(run);
        }
    }
    Ok(runs)
}

/// Whether `later` replaces `earlier`: a higher version always does, the same
/// version does when it was submitted afterwards (a rebuild), and a lower one
/// never does, however recently it was submitted. Ordering by submission time
/// alone let a release of 0.23.8 cancel a 0.23.10 already building and leave
/// the fleet on the older source.
pub(in crate::cli::release_submit) fn replaces(later: &ReleaseRun, earlier: &ReleaseRun) -> bool {
    match crate::cli::release_cmd::semver_order(&later.version, &earlier.version) {
        Ok(std::cmp::Ordering::Greater) => true,
        Ok(std::cmp::Ordering::Equal) => later.created_at > earlier.created_at,
        Ok(std::cmp::Ordering::Less) => false,
        // A run whose version is not SemVer was never admitted by submit;
        // between two such records only the submission order is known.
        Err(_) => later.created_at > earlier.created_at,
    }
}

/// Mark every live run of `newest`'s product and channel that it replaces
/// ([`replaces`]) superseded and cancel its builds that have not started.
/// Returns the ids it replaced.
pub(crate) async fn supersede_older(
    store: &JobStorage,
    newest: &ReleaseRun,
) -> Result<Vec<String>, CmdError> {
    let mut replaced = Vec::new();
    for mut run in live_runs_of(store, &newest.product, newest.channel).await? {
        if run.run_id == newest.run_id || !replaces(newest, &run) {
            continue;
        }
        let reason = format!(
            "superseded by release run {} ({} {})",
            newest.run_id, newest.product, newest.version
        );
        for platform in run.platforms.values_mut() {
            if platform.state == PlatformRunState::Failed || platform.job_id.is_empty() {
                continue;
            }
            // Only a build nobody has started is cancelled; a running build,
            // including one claimed between the read and the cancel, ends on
            // its own and `newer_than` refuses its publication.
            if cancel_queued_in_store(store, &platform.job_id).await? {
                platform.state = PlatformRunState::Failed;
                platform.failure = Some(reason.clone());
            }
        }
        run.state = ReleaseRunState::Superseded;
        run.failure = Some(reason);
        save(&mut run).await?;
        replaced.push(run.run_id);
    }
    Ok(replaced)
}

/// The id of a live run of the same product and channel that replaces `run`
/// ([`replaces`]), if any: the reason `run` must not publish now.
pub(crate) async fn newer_than(
    store: &JobStorage,
    run: &ReleaseRun,
) -> Result<Option<String>, CmdError> {
    let mut newest: Option<ReleaseRun> = None;
    for candidate in live_runs_of(store, &run.product, run.channel).await? {
        if candidate.run_id != run.run_id
            && replaces(&candidate, run)
            && newest.as_ref().is_none_or(|n| replaces(&candidate, n))
        {
            newest = Some(candidate);
        }
    }
    Ok(newest.map(|run| run.run_id))
}
