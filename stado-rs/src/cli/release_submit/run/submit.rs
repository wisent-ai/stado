//! `stado release submit` — the coordinator that walks one build through
//! every stage of the release pipeline.
//!
//! A release is made from a build. The run records the build it consumes
//! and reads that build's platform jobs; it queues no jobs of its own.

use std::collections::BTreeMap;

mod admission;
pub use admission::submit;

use crate::cli::release_cmd;
use crate::cli::release_submit::builds::jobs::platforms::{
    adopt_build, enqueue_platforms, reconcile_published, refresh_build,
};
use crate::cli::release_submit::deliver::deliveries::run_deliveries;
use crate::cli::release_submit::publish::artifact::publish;
use crate::cli::release_submit::publish::promotion::reconcile;
use crate::cli::release_submit::publish::signing::signing;
use crate::cli::release_submit::run::state::{load_build, persist_failure, save, save_build};
use crate::cli::release_submit::run::supersede::{newer_than, supersede_older};
use crate::cli::CmdError;
use crate::queue::storage::JobStorage;
use crate::release_pipeline::{
    BuildRunState, PlatformRunState, ReleasePipelineManifest, ReleaseRun, ReleaseRunState,
};

pub(super) async fn continue_run(
    mut run: ReleaseRun,
    m: ReleasePipelineManifest,
    json: bool,
    finish: bool,
) -> Result<(), CmdError> {
    run.failure = None;
    save(&mut run).await?;
    let store = match JobStorage::new().await {
        Ok(store) => store,
        Err(error) => {
            return Err(persist_failure(&mut run, CmdError::click(error.to_string())).await)
        }
    };
    // The build is where the jobs are. A run without one was submitted by a
    // release that queued jobs of its own and cannot be continued here; the
    // same source submitted again records a build and adopts it.
    let Some(build_id) = run.build_id.clone() else {
        return Err(persist_failure(
            &mut run,
            CmdError::click(format!(
                "release run {} predates build records and cannot be continued; submit the same commit again with `stado release submit --source`",
                run.run_id
            )),
        )
        .await);
    };
    let mut build = match load_build(&build_id).await? {
        Some(build) => build,
        None => {
            return Err(persist_failure(
                &mut run,
                CmdError::click(format!(
                    "build {build_id} of release run {} does not exist",
                    run.run_id
                )),
            )
            .await)
        }
    };
    let platforms: Vec<_> = m.platforms.keys().cloned().collect();
    // Retry what the build owes, then read what its jobs did: a platform
    // whose job failed or was cancelled gets a new job through the build,
    // and a passed one is read from its receipt. The run then takes the
    // build's platform records as its own, except the ones it has already
    // published.
    let mut enqueue_failure = enqueue_platforms(&store, &mut build, &m, &platforms).await?;
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
    // A platform still recorded as failed at this point was not re-enqueued:
    // its own enqueue failed, or the loop stopped at an earlier platform.
    // Publishing it would only re-read the terminal job of the previous
    // attempt and report that attempt's failure again, ahead of the enqueue
    // error that is the actual diagnosis. Measured on weles-worker 0.5.72 on
    // 2026-09-05: four resumes each reported the same upload timeout from a
    // job that had finished an hour earlier, while the reason nothing new was
    // built - no eligible builder, or a store timeout while staging inputs -
    // was never printed. Only platforms this run actually submitted are
    // published; the enqueue error is returned below.
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
    run.state = ReleaseRunState::Waiting;
    // A platform that could not be queued while others were is not a
    // finished submission: the run says so, and so does the operator's
    // terminal. On 2026-09-18 stado 0.21.7 queued only darwin-arm64 and
    // answered "builds queued" while linux-amd64 had found no builder.
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
            return Err(CmdError::click(format!(
                "release run {} is waiting on the platforms it could queue, but one was refused: {error}; `stado release resume {}` retries it",
                run.run_id, run.run_id
            )));
        }
        return Ok(());
    }
    // A run a newer submission has replaced is not published, even when its
    // builds ran to the end: the fleet wants the newest source, not every
    // source ten agents submitted in the same minute.
    if let Some(newer) = newer_than(&store, &run).await? {
        run.state = ReleaseRunState::Superseded;
        run.failure = Some(format!(
            "superseded by release run {newer} before publication"
        ));
        save(&mut run).await?;
        if json {
            println!("{}", serde_json::to_string_pretty(&run)?)
        } else {
            println!(
                "release run {} product={} version={} state={:?}: superseded by {newer}, not published",
                run.run_id, run.product, run.version, run.state
            )
        }
        return Ok(());
    }
    let mut signing_material = None;
    run.state = ReleaseRunState::Publishing;
    save(&mut run).await?;
    let mut artifacts = BTreeMap::new();
    for p in &submitted_platforms {
        let result = if run.platforms[p].state == PlatformRunState::Published {
            release_cmd::verified_artifact_for_submit(&run.product, &run.version, p).await
        } else {
            // Verification and delivery of already signed bytes need no private
            // signing grant on the machine resuming the release.
            if signing_material.is_none() {
                signing_material = Some(match signing(&run.product).await {
                    Ok(material) => material,
                    Err(error) => return Err(persist_failure(&mut run, error).await),
                });
            }
            let (key, private) = signing_material
                .as_ref()
                .expect("unpublished platform needs signing");
            publish(&mut run, &m, p, &store, key, private).await
        };
        let a = match result {
            Ok(artifact) => artifact,
            Err(error) => {
                let platform = run.platforms.get_mut(p).unwrap();
                platform.state = PlatformRunState::Failed;
                platform.failure = Some(error.to_string());
                if m.platforms[p].required {
                    return Err(persist_failure(&mut run, error).await);
                }
                save(&mut run).await?;
                continue;
            }
        };
        save(&mut run).await?;
        artifacts.insert(p.clone(), a);
    }
    if let Some(error) = enqueue_failure {
        return Err(persist_failure(&mut run, error).await);
    }
    run.state = ReleaseRunState::Delivering;
    save(&mut run).await?;
    if let Err(error) = run_deliveries(&mut run, &m, &artifacts).await {
        return Err(persist_failure(&mut run, error).await);
    }
    if m.promotion.reconcile {
        if let Err(error) =
            release_cmd::promote_for_submit(&run.product, &run.version, run.channel).await
        {
            return Err(persist_failure(&mut run, error).await);
        }
        if let Err(error) = reconcile(&run).await {
            return Err(persist_failure(&mut run, error).await);
        }
        run.state = ReleaseRunState::Reconciled
    } else {
        run.state = ReleaseRunState::Completed
    }
    save(&mut run).await?;
    if json {
        println!("{}", serde_json::to_string_pretty(&run)?)
    } else {
        println!(
            "release run {} product={} version={} state={:?}",
            run.run_id, run.product, run.version, run.state
        )
    }
    Ok(())
}
