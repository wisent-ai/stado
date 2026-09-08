//! Reconcile every declared platform of one run with the world: recover an
//! already-published coordinate, re-open a stale publication, and enqueue the
//! builds that are still owed.

use crate::cli::release_cmd;
use crate::cli::release_submit::builds::jobs::enqueue::enqueue;
use crate::cli::release_submit::builds::jobs::terminal::read_terminal_job;
use crate::cli::release_submit::run::state::save;
use crate::cli::storage;
use crate::cli::CmdError;
use crate::models::job_state;
use crate::queue::storage::JobStorage;
use crate::release_control;
use crate::release_pipeline::{PlatformRunState, ReleasePipelineManifest, ReleaseRun};

/// The enqueue failure that stopped the walk, if one did: the caller reports
/// it after publishing the platforms this run did submit.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn enqueue_platforms(
    store: &JobStorage,
    run: &mut ReleaseRun,
    m: &ReleasePipelineManifest,
    version: &str,
    id: &str,
    commit: &str,
    source_sha: &str,
    source_input_uri: &str,
    manifest_sha: &str,
    manifest_uri: &str,
    platforms: &[String],
) -> Result<Option<CmdError>, CmdError> {
    let mut enqueue_failure = None;
    for p in platforms {
        if run.platforms.contains_key(p) {
            let base =
                release_control::release_base(&m.product, version, p).map_err(CmdError::click)?;
            let manifest_uri = format!("{base}/{}", release_control::RELEASE_MANIFEST_NAME);
            // release.json is the coordinate's commit marker. A process may
            // publish it and lose the next status write; rebuilding then creates
            // a different qualification timestamp and collides with the
            // immutable coordinate. Recover only after the complete signed
            // coordinate verifies and names this exact source revision.
            if storage::release_object_present(&manifest_uri).await? {
                let artifact =
                    release_cmd::verified_artifact_for_submit(&m.product, version, p).await?;
                if artifact.source_revision != commit {
                    return Err(CmdError::click(format!(
                        "published release {p} names source revision {}, expected {commit}",
                        artifact.source_revision
                    )));
                }
                {
                    let platform = run.platforms.get_mut(p).expect("checked above");
                    platform.state = PlatformRunState::Published;
                    platform.artifact_sha256 = Some(artifact.artifact_sha256);
                    platform.release_manifest_sha256 = Some(artifact.manifest_sha256);
                    platform.qualification_uri = Some(format!(
                        "{base}/{}",
                        release_control::RELEASE_QUALIFICATION_NAME
                    ));
                    platform.failure = None;
                }
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
        if run
            .platforms
            .get(p)
            .is_some_and(|platform| platform.state == PlatformRunState::Failed)
        {
            // A failed publication read is not a failed build. Inspect the
            // original job before deriving another build identity.
            let job_id = &run.platforms[p].job_id;
            let retry = match read_terminal_job(store, job_id).await? {
                Some(job) => matches!(job.state.as_str(), job_state::FAILED | job_state::CANCELLED),
                None => {
                    if store.read_job("running", job_id).await?.is_none()
                        && store.read_job("queue", job_id).await?.is_none()
                    {
                        return Err(CmdError::click(format!(
                            "release job {job_id} was not found in recorded states; \
                             refusing a replacement without terminal failure"
                        )));
                    }
                    false
                }
            };
            if !retry {
                let platform = run.platforms.get_mut(p).expect("checked above");
                platform.state = PlatformRunState::Submitted;
                platform.failure = None;
                save(run).await?;
            }
        }
        if !run.platforms.contains_key(p) || run.platforms[p].state == PlatformRunState::Failed {
            let prior_terminal_job_id = run
                .platforms
                .get(p)
                .filter(|platform| platform.state == PlatformRunState::Failed)
                .map(|platform| platform.job_id.as_str());
            let r = match enqueue(
                store,
                id,
                m,
                version,
                p,
                commit,
                source_sha,
                source_input_uri,
                manifest_sha,
                manifest_uri,
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
            run.platforms.insert(p.clone(), r);
            save(run).await?
        }
    }
    Ok(enqueue_failure)
}
