//! `stado release submit` — the coordinator that walks one build through
//! every stage of the release pipeline.
//!
//! A release is made from a build. The run records the build it consumes
//! and reads that build's platform jobs; it queues no jobs of its own.

mod admission;
mod publication;
pub use admission::submit;

use crate::cli::release_submit::builds::jobs::platforms::{
    adopt_build, enqueue_platforms, reconcile_published,
};
use crate::cli::release_submit::builds::jobs::terminal::refresh_build;
use crate::cli::release_submit::run::state::{load_build, persist_failure, save, save_build};
use crate::cli::release_submit::run::supersede::{
    newer_than, published_newer_than, supersede_older,
};
use crate::cli::CmdError;
use crate::queue::storage::JobStorage;
use crate::release_pipeline::{
    BuildRunState, PlatformRunState, ReleasePipelineManifest, ReleaseRun, ReleaseRunState,
};

use publication::publish_and_deliver;

pub(super) async fn continue_run(
    mut run: ReleaseRun,
    m: ReleasePipelineManifest,
    json: bool,
    finish: bool,
    retry_failed: bool,
) -> Result<(), CmdError> {
    run.failure = None;
    save(&mut run).await?;
    let store = match JobStorage::new().await {
        Ok(store) => store,
        Err(error) => return Err(persist_failure(&mut run, CmdError::from(error)).await),
    };
    let placement = crate::cli::release_submit::deliver::placement::prepare(&store, &run, &m);
    let deliveries = match placement.await {
        Ok(deliveries) => deliveries,
        Err(error) => return Err(persist_failure(&mut run, error).await),
    };
    // The build is where the jobs are. A run without one was submitted by a
    // release that queued jobs of its own and cannot be continued here; the
    // same source submitted again records a build and adopts it.
    let Some(build_id) = run.build_id.clone() else {
        let error = CmdError::click(format!(
            "release run {} predates build records and cannot be continued; submit the same commit again with `stado release submit --source`",
            run.run_id
        ))
        .stating(crate::primitives::failure::FailureCode::Refused);
        return Err(persist_failure(&mut run, error).await);
    };
    let mut build = match load_build(&build_id).await? {
        Some(build) => build,
        None => {
            let error = CmdError::click(format!(
                "build {build_id} of release run {} does not exist",
                run.run_id
            ))
            .stating(crate::primitives::failure::FailureCode::NotFound);
            return Err(persist_failure(&mut run, error).await);
        }
    };
    let platforms: Vec<_> = m.platforms.keys().cloned().collect();
    // Retry what the build owes, then read what its jobs did: a platform
    // whose job failed or was cancelled gets a new job through the build,
    // and a passed one is read from its receipt. The run then takes the
    // build's platform records as its own, except the ones it has already
    // published.
    let enqueue = enqueue_platforms(&store, &mut build, &m, &platforms, retry_failed);
    let mut enqueue_failure = enqueue.await?;
    refresh_build(&store, &mut build, &m).await?;
    // The build's own record says what the run is about to say: nothing
    // queued is a failed build, a platform refused is a build still waiting
    // on the rest.
    match &enqueue_failure {
        Some(error) => {
            build.failure = Some(format!("not every platform was queued: {error}"));
            if build
                .platforms
                .values()
                .all(|platform| platform.state == PlatformRunState::Failed)
            {
                build.state = BuildRunState::Failed;
            }
        }
        None => build.failure = None,
    }
    save_build(&mut build).await?;
    reconcile_published(&mut run, &platforms).await?;
    adopt_build(&mut run, &build);
    save(&mut run).await?;
    // A platform still marked failed was not re-enqueued. Publish only the
    // platforms submitted by this attempt, otherwise an old terminal job's
    // failure would obscure the current enqueue error returned below.
    let submitted_platforms: Vec<_> = platforms
        .iter()
        .filter(|platform| {
            run.platforms
                .get(*platform)
                .is_some_and(|record| record.state != PlatformRunState::Failed)
        })
        .cloned()
        .collect();
    // Nothing was submitted and the walk stopped: that failure IS the
    // diagnosis, so it is reported before this run reaches for signing
    // material. Reaching for it first replaces the reason nothing was built
    // with an unrelated one -- on a machine without the release signing
    // grant, `no live fleet builder can CLAIM release_platform ...` became
    // `cannot read signing key ...` in both the operator's error and the
    // durable run document, which is the same defect the comment above
    // describes, one stage later.
    if submitted_platforms.is_empty() {
        if let Some(error) = enqueue_failure.take() {
            return Err(persist_failure(&mut run, error).await);
        }
    }
    // A pass over a run whose platforms are all published resumes its
    // deliveries; walking it back through `waiting` and `publishing` made
    // every delivery worker that started mid-walk refuse its job ("the run is
    // Publishing, not delivering"), on every tick of the release agent.
    // `reconcile_published` has just read every published coordinate back
    // and verified it, so such a run is put back into `delivering` here, in
    // the same write that clears its failure. A run left `failed` until the
    // deliveries were requeued made every delivery its host claimed in
    // between refuse itself ("the run is Failed, not delivering"), and the
    // resume that was meant to deliver it failed on those refusals.
    let already_published = !submitted_platforms.is_empty()
        && submitted_platforms
            .iter()
            .all(|platform| run.platforms[platform].state == PlatformRunState::Published);
    run.state = if finish && already_published {
        ReleaseRunState::Delivering
    } else {
        ReleaseRunState::Waiting
    };
    // A platform that could not be queued while others were is not a
    // finished submission: the run says so, and so does the operator's
    // terminal, instead of answering "builds queued" when one platform has
    // found no builder.
    if let Some(error) = &enqueue_failure {
        run.failure = Some(format!("not every platform was queued: {error}"));
    }
    save(&mut run).await?;
    if !finish {
        // One run per product and channel is worth building: now that this
        // run has builds queued, the older live runs lose theirs and are not
        // published later. A submission that queued nothing supersedes
        // nothing - a run no builder will take must not take the fleet's
        // release away from the run that is building.
        for replaced in supersede_older(&store, &run).await? {
            eprintln!("release run {replaced} superseded by {}", run.run_id);
        }
        if json {
            println!("{}", serde_json::to_string_pretty(&run)?)
        } else {
            let standing = match build.state {
                BuildRunState::Passed => "build passed",
                BuildRunState::Failed => "build failed",
                BuildRunState::Waiting => "builds queued",
            };
            println!(
                "release run {} product={} version={} build={} state={:?}: {standing}; the control host's release agent publishes and delivers once the build has passed, `stado release status {}` follows it",
                run.run_id, run.product, run.version, build.build_id, run.state, run.product
            )
        }
        if let Some(error) = enqueue_failure {
            return Err(error
                .within(format!(
                    "release run {} is waiting on the platforms it could queue, but one was refused",
                    run.run_id
                ))
                .also(format!("`stado release resume {}` retries it", run.run_id)));
        }
        return Ok(());
    }
    // A required platform whose build failed and is not built again ends the
    // run on what its job wrote; one still queued or building leaves the run
    // waiting for the pass that finds it ended. Publishing either read an
    // unfinished job as a failed one and recorded "still queued" in place of
    // the build's own failure.
    if let Some((platform, failure)) = m
        .platforms
        .iter()
        .filter(|(_, recipe)| recipe.required)
        .find_map(|(platform, _)| {
            build
                .platforms
                .get(platform)
                .filter(|record| record.state == PlatformRunState::Failed)
                .map(|record| (platform.clone(), record.failure.clone()))
        })
    {
        let error = CmdError::click(format!(
            "{platform}: {}; `stado release resume {}` builds it again",
            failure.unwrap_or_else(|| "the build failed".to_string()),
            run.run_id
        ))
        .stating(crate::primitives::failure::FailureCode::Refused);
        return Err(persist_failure(&mut run, error).await);
    }
    if build.state == BuildRunState::Waiting {
        if json {
            println!("{}", serde_json::to_string_pretty(&run)?)
        } else {
            println!(
                "release run {} product={} version={} state={:?}: builds still queued or running; \
                 the next release agent tick or `stado release resume {}` publishes once they pass",
                run.run_id, run.product, run.version, run.state, run.run_id
            )
        }
        return Ok(());
    }
    // A run a newer submission has replaced is not published, even when its
    // builds ran to the end: the fleet wants the newest source, not every
    // source ten agents submitted in the same minute. One that had already
    // published every platform stops delivering only for a newer run that has
    // published too: a newer run still building may fail, and then nothing
    // would deliver. Its bytes stay published either way.
    let newer = if already_published {
        published_newer_than(&store, &run).await?
    } else {
        newer_than(&store, &run).await?
    };
    if let Some(newer) = newer {
        run.state = ReleaseRunState::Superseded;
        run.failure = Some(if already_published {
            format!(
                "superseded by release run {newer} after publication; {newer} delivers from here"
            )
        } else {
            format!("superseded by release run {newer} before publication")
        });
        save(&mut run).await?;
        if json {
            println!("{}", serde_json::to_string_pretty(&run)?)
        } else {
            println!(
                "release run {} product={} version={} state={:?}: {}",
                run.run_id,
                run.product,
                run.version,
                run.state,
                run.failure.as_deref().unwrap_or_default()
            )
        }
        return Ok(());
    }
    publish_and_deliver(
        &mut run,
        &m,
        &store,
        &deliveries,
        &submitted_platforms,
        already_published,
        enqueue_failure,
        json,
    )
    .await
}
