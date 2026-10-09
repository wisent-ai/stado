//! Reconcile every declared platform with the world. The build owns the
//! jobs: it enqueues the builds still owed and re-enqueues the ones that
//! ended badly. The release owns the coordinates: it recovers an
//! already-published one and re-opens a stale publication.

use crate::cli::release_cmd;
use crate::cli::release_submit::builds::builder::Fleet;
use crate::cli::release_submit::builds::jobs::enqueue::enqueue;
use crate::cli::release_submit::builds::jobs::terminal::{read_terminal_job, settled};
use crate::cli::release_submit::run::source::build_uri;
use crate::cli::release_submit::run::state::{save, save_build};
use crate::cli::storage;
use crate::cli::CmdError;
use crate::models::job_state;
use crate::queue::storage::JobStorage;
use crate::release_control;
use crate::release_pipeline::{BuildRun, PlatformRunState, ReleasePipelineManifest, ReleaseRun};

/// Enqueue every platform of the build that has no live or passed job.
///
/// A platform whose job ended failed or cancelled is built again only when
/// `retry_failed` says a person asked for it — a submission or `stado release
/// resume`. The release agent's own pass does not: a build that failed its
/// format check fails it again, and rebuilding it on every tick replaced the
/// failure with the next job's "still queued" instead of ending the run on
/// what the job wrote.
///
/// The enqueue failure that stopped the walk, if one did, is returned rather
/// than raised: the caller records what was queued before reporting it.
pub(crate) async fn enqueue_platforms(
    store: &JobStorage,
    build: &mut BuildRun,
    m: &ReleasePipelineManifest,
    platforms: &[String],
    retry_failed: bool,
) -> Result<Option<CmdError>, CmdError> {
    let source_input_uri = build_uri(&build.product, &build.build_id, "inputs/source.tar.gz");
    let mut enqueue_failure = None;
    // Read on the first platform that needs a job, then shared by the rest.
    let mut fleet: Option<Fleet> = None;
    for p in platforms {
        // A build whose record lost a platform's job (an enqueue interrupted
        // after the queue admitted the job, before the record was saved)
        // must not submit the same first attempt again: its run already
        // planned that job, and a request derived anew — another builder,
        // another Stado's worker command — cannot match it, so every
        // resubmission was refused with "a planned job not derivable from
        // its request" or "durable prior admission exists". The planned job
        // is recorded again and judged below like any recorded job.
        if !build.platforms.contains_key(p) {
            if let Some(run) = first_submission(store, build, p).await? {
                build.platforms.insert(p.clone(), run);
                save_build(build).await?;
            }
        }
        // A platform stays recorded as Submitted while its job runs, and
        // nothing wrote Failed when the job ended badly. A resubmission then
        // saw Submitted, kept the dead job, and reported the run as waiting
        // for builds that would never come: a run sits on a cancelled job and
        // a failed job for hours while every resubmission answers "builds
        // queued". The job
        // is the truth; a terminal failure or cancellation behind Submitted
        // is a failed platform.
        if build
            .platforms
            .get(p)
            .is_some_and(|platform| platform.state == PlatformRunState::Submitted)
        {
            let job_id = build.platforms[p].job_id.clone();
            if let Some(reason) = super::fallback::release_silent_placement(store, &job_id).await? {
                let platform = build.platforms.get_mut(p).expect("checked above");
                platform.state = PlatformRunState::Failed;
                platform.failure = Some(reason);
                save_build(build).await?;
            } else if let Some(job) = read_terminal_job(store, &job_id).await? {
                if matches!(job.state.as_str(), job_state::FAILED | job_state::CANCELLED) {
                    let failure = if retry_failed {
                        format!(
                            "build job {job_id} ended {}; a new build is enqueued in its place",
                            job.state
                        )
                    } else {
                        format!(
                            "build job {job_id} ended {}{}",
                            job.state,
                            super::terminal::job_output_tail(store, &job_id).await
                        )
                    };
                    let platform = build.platforms.get_mut(p).expect("checked above");
                    platform.state = PlatformRunState::Failed;
                    platform.failure = Some(failure);
                    save_build(build).await?;
                }
            }
        }
        if build
            .platforms
            .get(p)
            .is_some_and(|platform| platform.state == PlatformRunState::Failed)
        {
            // A failed publication read is not a failed build. Inspect the
            // original job before deriving another build identity.
            let job_id = &build.platforms[p].job_id;
            // A job that ran and failed fails again from the same source; a
            // cancelled one (a silent placement handed to another builder, a
            // superseded run) never ran, so the agent may build it again. The
            // run reaper deletes a settled job's record; what the job did is
            // kept in its receipt, in its run's retained outcome or in its
            // transition record, and any of them is the terminal failure a
            // replacement needs. Refusing because the record was gone left a
            // failed release unresumable once its job had been reaped.
            let ended = match settled(store, job_id).await? {
                Some(job) => Some(job.state),
                None if store.read_job("running", job_id).await?.is_some() => None,
                None if store.read_job("queue", job_id).await?.is_some() => None,
                None => {
                    return Err(CmdError::refused(format!(
                        "build job {job_id} is in no queue state, left no receipt, no reaped \
                         run retains it and no transition record names how it ended; refusing \
                         a replacement without evidence of how it ended"
                    )));
                }
            };
            let retry = ended
                .as_deref()
                .is_some_and(|state| matches!(state, job_state::FAILED | job_state::CANCELLED));
            let ran_and_failed = ended.as_deref() == Some(job_state::FAILED);
            if !retry {
                let platform = build.platforms.get_mut(p).expect("checked above");
                platform.state = PlatformRunState::Submitted;
                platform.failure = None;
                save_build(build).await?;
            } else if ran_and_failed && !retry_failed {
                continue;
            }
        }
        if !build.platforms.contains_key(p) || build.platforms[p].state == PlatformRunState::Failed
        {
            let prior_terminal_job_id = build
                .platforms
                .get(p)
                .filter(|platform| platform.state == PlatformRunState::Failed)
                .map(|platform| platform.job_id.as_str());
            if fleet.is_none() {
                match Fleet::read().await {
                    Ok(read) => fleet = Some(read),
                    Err(error) => {
                        enqueue_failure = Some(error);
                        break;
                    }
                }
            }
            let r = match enqueue(
                store,
                fleet.as_ref().expect("read above"),
                &build.build_id,
                m,
                &build.version,
                p,
                &build.source_commit,
                &build.source_sha256,
                &source_input_uri,
                &build.manifest_sha256,
                &build.manifest_uri,
                prior_terminal_job_id,
            )
            .await
            {
                Ok(run) => run,
                Err(error) => {
                    enqueue_failure = Some(error);
                    break;
                }
            };
            build.platforms.insert(p.clone(), r);
            save_build(build).await?
        }
    }
    Ok(enqueue_failure)
}

/// The job the build's first submission of `platform` planned, as a platform
/// record, when that submission exists: its run manifest names the job and
/// the saved worker request names the builder it was placed on. `None` when
/// the build never submitted the platform.
async fn first_submission(
    store: &JobStorage,
    build: &BuildRun,
    platform: &str,
) -> Result<Option<crate::release_pipeline::PlatformRun>, CmdError> {
    let run_id = crate::queue::submit::stable_run_id(
        super::RELEASE_BUILD_RUN_SCOPE,
        &format!("{}\0{platform}", build.build_id),
    );
    let Some(manifest) = crate::queue::runs::read_run(store, &run_id).await? else {
        return Ok(None);
    };
    let job_id = manifest
        .get("entries")
        .and_then(serde_json::Value::as_array)
        .and_then(|entries| entries.first())
        .and_then(|entry| entry.get("job_id"))
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| {
            CmdError::click(format!(
                "build {}'s submission {run_id} of {platform} names no job",
                build.build_id
            ))
            .stating(crate::primitives::failure::FailureCode::InfraDown)
        })?
        .to_string();
    let request_path = crate::cli::release_submit::build_path(
        &build.product,
        &build.build_id,
        &format!("requests/{platform}.json"),
    );
    let request: crate::release_pipeline::WorkerRequest =
        match store.read_bytes(&request_path).await? {
            Some(bytes) => serde_json::from_slice(&bytes)?,
            None => {
                return Err(CmdError::click(format!(
                    "build {}'s submission {run_id} planned {job_id}, but its worker request \
                 {request_path} is gone, so the builder it was placed on is unknown",
                    build.build_id
                ))
                .stating(crate::primitives::failure::FailureCode::InfraDown))
            }
        };
    Ok(Some(crate::release_pipeline::PlatformRun {
        platform: platform.into(),
        builder: request.builder,
        output_prefix: format!("status/{job_id}/output/"),
        job_id,
        state: PlatformRunState::Submitted,
        artifact_sha256: None,
        release_manifest_sha256: None,
        qualification_uri: None,
        failure: None,
    }))
}

/// Bring the run's view of each platform's coordinate in line with the
/// store, before the build's platform records are mirrored onto it.
pub(crate) async fn reconcile_published(
    run: &mut ReleaseRun,
    platforms: &[String],
) -> Result<(), CmdError> {
    for p in platforms {
        if !run.platforms.contains_key(p) {
            continue;
        }
        // The run was admitted with valid coordinates, so one that fails now
        // is a damaged run record.
        let base = release_control::release_base(&run.product, &run.version, p)
            .map_err(CmdError::unreachable)?;
        let manifest_uri = format!("{base}/{}", release_control::RELEASE_MANIFEST_NAME);
        // release.json is the coordinate's commit marker. A process may
        // publish it and lose the next status write; rebuilding then creates
        // a different qualification timestamp and collides with the
        // immutable coordinate. Recover only after the complete signed
        // coordinate verifies and names this exact source revision.
        if storage::release_object_present(&manifest_uri).await? {
            let artifact =
                release_cmd::verified_artifact_for_submit(&run.product, &run.version, p).await?;
            if artifact.source_revision != run.source_commit {
                return Err(CmdError::click(format!(
                    "published release {p} names source revision {}, expected {}",
                    artifact.source_revision, run.source_commit
                ))
                .stating(crate::primitives::failure::FailureCode::InfraDown));
            }
            let platform = run.platforms.get_mut(p).expect("checked above");
            platform.state = PlatformRunState::Published;
            platform.artifact_sha256 = Some(artifact.artifact_sha256);
            platform.release_manifest_sha256 = Some(artifact.manifest_sha256);
            platform.qualification_uri = Some(format!(
                "{base}/{}",
                release_control::RELEASE_QUALIFICATION_NAME
            ));
            platform.failure = None;
            save(run).await?;
            continue;
        }
        // A run may say Published while its coordinate was written through
        // an obsolete object origin. The durable qualification job is still
        // the source of truth; republish it instead of treating absent
        // release.json as a committed release.
        if run.platforms[p].state == PlatformRunState::Published {
            let platform = run.platforms.get_mut(p).expect("checked above");
            platform.state = PlatformRunState::Qualified;
            platform.artifact_sha256 = None;
            platform.release_manifest_sha256 = None;
            platform.qualification_uri = None;
            platform.failure = None;
            save(run).await?;
        }
    }
    Ok(())
}

/// The run takes the build's platform records as its own, except where it
/// has already published a platform: a published coordinate is the run's
/// fact, not the build's.
pub(crate) fn adopt_build(run: &mut ReleaseRun, build: &BuildRun) {
    for (name, platform) in &build.platforms {
        if run
            .platforms
            .get(name)
            .is_some_and(|mine| mine.state == PlatformRunState::Published)
        {
            continue;
        }
        run.platforms.insert(name.clone(), platform.clone());
    }
}
