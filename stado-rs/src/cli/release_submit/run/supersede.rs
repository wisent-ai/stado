//! One product, one channel, one run worth building at a time.
//!
//! Ten agents submitting every few seconds used to queue ten runs and twenty
//! builds, and every one of them was built, signed and published; only the
//! delivery fence (`latest_submitted_run`) kept the older ones off the hosts.
//! Here a newer submission of the same product and channel supersedes the
//! older live runs: their builds still waiting in the queue are cancelled,
//! and a build already running is left to end but is not published, because
//! by then a newer run exists. The run says which one replaced it.

use crate::cli::release_submit::run::state::save;
use crate::cli::work::cancel::cancel_in_store;
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

/// Mark every older live run of `newest`'s product and channel superseded
/// and cancel its builds that have not started. Returns the ids it replaced.
pub(crate) async fn supersede_older(
    store: &JobStorage,
    newest: &ReleaseRun,
) -> Result<Vec<String>, CmdError> {
    let mut replaced = Vec::new();
    for mut run in live_runs_of(store, &newest.product, newest.channel).await? {
        if run.run_id == newest.run_id || run.created_at >= newest.created_at {
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
            // Only a build nobody has started is cancelled; a running build
            // ends on its own and `newer_than` refuses its publication.
            if store
                .read_job("queue", &platform.job_id)
                .await
                .map_err(|error| CmdError::click(error.to_string()))?
                .is_some()
            {
                cancel_in_store(store, &platform.job_id).await?;
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

/// The id of a newer run of the same product and channel that is still live
/// or has published, if any: the reason `run` must not publish now.
pub(crate) async fn newer_than(
    store: &JobStorage,
    run: &ReleaseRun,
) -> Result<Option<String>, CmdError> {
    let mut newest: Option<ReleaseRun> = None;
    for candidate in live_runs_of(store, &run.product, run.channel).await? {
        if candidate.run_id != run.run_id
            && candidate.created_at > run.created_at
            && newest
                .as_ref()
                .is_none_or(|n| candidate.created_at > n.created_at)
        {
            newest = Some(candidate);
        }
    }
    Ok(newest.map(|run| run.run_id))
}
