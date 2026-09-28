//! Reconcile every declared platform with the world. The build owns the
//! jobs: it enqueues the builds still owed and re-enqueues the ones that
//! ended badly. The release owns the coordinates: it recovers an
//! already-published one and re-opens a stale publication.

use crate::cli::release_cmd;
use crate::cli::release_submit::builds::builder::Fleet;
use crate::cli::release_submit::builds::jobs::enqueue::enqueue;
use crate::cli::release_submit::builds::jobs::terminal::read_terminal_job;
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
/// The enqueue failure that stopped the walk, if one did, is returned rather
/// than raised: the caller records what was queued before reporting it.
pub(crate) async fn enqueue_platforms(
    store: &JobStorage,
    build: &mut BuildRun,
    m: &ReleasePipelineManifest,
    platforms: &[String],
) -> Result<Option<CmdError>, CmdError> {
    let source_input_uri = build_uri(&build.product, &build.build_id, "inputs/source.tar.gz");
    let mut enqueue_failure = None;
    // Read on the first platform that needs a job, then shared by the rest.
    let mut fleet: Option<Fleet> = None;
    for p in platforms {
        // A platform stays recorded as Submitted while its job runs, and
        // nothing wrote Failed when the job ended badly. A resubmission then
        // saw Submitted, kept the dead job, and reported the run as waiting
        // for builds that would never come: jeden 0.1.8 sat on a cancelled
        // darwin job and a failed linux job for three hours on 2026-09-18
        // while three resubmissions each answered "builds queued". The job
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
                    let platform = build.platforms.get_mut(p).expect("checked above");
                    platform.state = PlatformRunState::Failed;
                    platform.failure = Some(format!(
                        "build job {job_id} ended {}; a new build is enqueued in its place",
                        job.state
                    ));
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
            let retry = match read_terminal_job(store, job_id).await? {
                Some(job) => matches!(job.state.as_str(), job_state::FAILED | job_state::CANCELLED),
                None => {
                    if store.read_job("running", job_id).await?.is_none()
                        && store.read_job("queue", job_id).await?.is_none()
                    {
                        return Err(CmdError::click(format!(
                            "build job {job_id} was not found in recorded states; \
                             refusing a replacement without terminal failure"
                        )));
                    }
                    false
                }
            };
            if !retry {
                let platform = build.platforms.get_mut(p).expect("checked above");
                platform.state = PlatformRunState::Submitted;
                platform.failure = None;
                save_build(build).await?;
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
        let base = release_control::release_base(&run.product, &run.version, p)
            .map_err(CmdError::click)?;
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
                )));
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
