//! One product, one channel, one run worth building at a time.
//!
//! Ten agents submitting every few seconds used to queue ten runs and twenty
//! builds, and every one of them was built, signed and published; only the
//! delivery fence (`latest_submitted_run`) kept the older ones off the hosts.
//! Here a submission of the same product and channel supersedes the live runs
//! it replaces ([`replaces`]: a lower version, or the same version submitted
//! earlier): their builds are cancelled, queued or running, and the run says
//! which one replaced it. A running build was once left to end, unpublished:
//! it kept the builder's Cargo directory for that product the whole time, so
//! the build replacing it was declined there (`a stado darwin-arm64 build is
//! already running here and holds the Cargo build directory`) until a build
//! nobody would publish had finished.

use crate::cli::release_submit::run::state::save;
use crate::cli::work::cancel::{cancel_claimed_in_store, cancel_queued_in_store};
use crate::cli::CmdError;
use crate::queue::storage::JobStorage;
use crate::release_pipeline::{PlatformRunState, ReleaseRun, ReleaseRunState};

/// Runs of the same product and channel whose state `wanted` accepts.
async fn runs_of(
    store: &JobStorage,
    product: &str,
    channel: crate::release_pipeline::PipelineChannel,
    wanted: fn(&ReleaseRunState) -> bool,
) -> Result<Vec<ReleaseRun>, CmdError> {
    let mut runs = Vec::new();
    for path in store
        .list_paths("runs/release-pipeline/", 0)
        .await
        .map_err(CmdError::from)?
    {
        if !path.ends_with("/run.json") {
            continue;
        }
        let Some(text) = store.download_text(&path).await.map_err(CmdError::from)? else {
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
        if run.product == product && run.channel == channel && wanted(&run.state) {
            runs.push(run);
        }
    }
    Ok(runs)
}

/// Runs that have not ended and have not published: `Failed` and
/// `Superseded` runs are already over; `Delivering`, `Completed`, `Promoted`
/// and `Reconciled` runs have published and stay.
async fn live_runs_of(
    store: &JobStorage,
    product: &str,
    channel: crate::release_pipeline::PipelineChannel,
) -> Result<Vec<ReleaseRun>, CmdError> {
    runs_of(store, product, channel, |state| {
        matches!(
            state,
            ReleaseRunState::Submitting | ReleaseRunState::Waiting | ReleaseRunState::Publishing
        )
    })
    .await
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
            // A queued build leaves the queue; a claimed one is cancelled the
            // way `stado cancel` cancels it, so its agent stops it and frees
            // the builder for the run that replaces it.
            if !cancel_queued_in_store(store, &platform.job_id).await? {
                cancel_claimed_in_store(store, &platform.job_id).await?;
            }
            platform.state = PlatformRunState::Failed;
            platform.failure = Some(reason.clone());
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

/// The id of a run of the same product and channel that replaces `run` and
/// has itself published, if any: the only reason an already published `run`
/// stops delivering.
///
/// A run still building is no such reason. Stopping a published run for one
/// left the fleet with nothing to deliver whenever that build then failed:
/// stado 0.23.29 was published and still owed the laptop its copy when a
/// queued 0.23.30 superseded it, 0.23.30 failed on darwin, and no run
/// delivered anything.
pub(crate) async fn published_newer_than(
    store: &JobStorage,
    run: &ReleaseRun,
) -> Result<Option<String>, CmdError> {
    let published = runs_of(store, &run.product, run.channel, |state| {
        matches!(
            state,
            ReleaseRunState::Delivering
                | ReleaseRunState::Completed
                | ReleaseRunState::Promoted
                | ReleaseRunState::Reconciled
        )
    })
    .await?;
    let mut newest: Option<ReleaseRun> = None;
    for candidate in published {
        if candidate.run_id != run.run_id
            && replaces(&candidate, run)
            && newest.as_ref().is_none_or(|n| replaces(&candidate, n))
        {
            newest = Some(candidate);
        }
    }
    Ok(newest.map(|run| run.run_id))
}
