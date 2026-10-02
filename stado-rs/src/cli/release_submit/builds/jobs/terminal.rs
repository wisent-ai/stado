//! One release job's terminal record, the evidence its own log carries when
//! it failed, and what the build makes of the terminal jobs it finds.

use crate::cli::CmdError;
use crate::models::{job_state, Job};
use crate::queue::storage::JobStorage;
use crate::release_pipeline::{
    BuildReceipt, BuildRun, BuildRunState, PlatformRunState, ReleasePipelineManifest, StepStatus,
};

pub(crate) async fn read_terminal_job(
    store: &JobStorage,
    id: &str,
) -> Result<Option<Job>, CmdError> {
    for prefix in crate::queue::runs::TERMINAL_PREFIXES {
        if let Some(job) = store.read_job(prefix, id).await? {
            return Ok(Some(job));
        }
    }
    Ok(None)
}

/// The release job's terminal record. A job that is still queued or running
/// is an error naming where it is and, when queued, the host it is pinned to.
///
/// A terminal record the queue has since retired — the run reaper settles
/// terminal jobs and deletes their blobs on its own cadence, and a release is
/// finished on another host's cadence — is answered by the job's own
/// receipt, which the worker wrote with its exit and which every publish
/// verifies in full (run, job, builder, source, status, archive digest)
/// before a byte is published. A job with neither record nor receipt has not
/// reached a terminal state.
pub(crate) async fn terminal(store: &JobStorage, id: &str) -> Result<Job, CmdError> {
    match ended(store, id).await? {
        Ended::Job(job) => Ok(*job),
        Ended::Queued { state, host } => Err(CmdError::click(format!(
            "release job {id} is still queued ({state}) on {host}; no host has claimed it. The \
             host's own decline is in its agent log: read it with `stado service logs <unit> \
             --host <host>`"
        ))),
        Ended::Running => Err(CmdError::click(format!(
            "release job {id} is still running"
        ))),
    }
}

/// Where one release job stands: ended, or not yet.
pub(crate) enum Ended {
    /// The job's terminal record, or the job its receipt describes.
    Job(Box<Job>),
    /// Queued and unclaimed, on the host it is pinned to.
    Queued {
        state: String,
        host: String,
    },
    Running,
}

/// The release job's terminal record, its receipt, or where it still is.
/// A delivery pass that finds a job queued or running leaves the run
/// delivering and reads it again on a later pass; nothing here waits.
pub(crate) async fn ended(store: &JobStorage, id: &str) -> Result<Ended, CmdError> {
    if let Some(job) = read_terminal_job(store, id).await? {
        return Ok(Ended::Job(Box::new(job)));
    }
    if let Some(queued) = store.read_job("queue", id).await? {
        let host = if queued.pinned_host.is_empty() {
            "no pinned host".to_string()
        } else {
            queued.pinned_host
        };
        return Ok(Ended::Queued {
            state: queued.state,
            host,
        });
    }
    if store.read_job("running", id).await?.is_some() {
        return Ok(Ended::Running);
    }
    if let Some(bytes) = store
        .read_bytes(&format!("status/{id}/output/receipt.json"))
        .await?
    {
        let receipt: BuildReceipt = serde_json::from_slice(&bytes)?;
        let state = if receipt.status == StepStatus::Passed {
            job_state::COMPLETED
        } else {
            job_state::FAILED
        };
        return Ok(Ended::Job(Box::new(Job {
            job_id: id.to_string(),
            pinned_host: receipt.builder,
            state: state.to_string(),
            ..Job::default()
        })));
    }
    Err(CmdError::click(format!(
        "release job {id} has not reached a terminal state, and left no receipt"
    )))
}

/// The last lines the failed job wrote, so the failure carries its own
/// evidence.
///
/// A failed release job used to surface only the queue's one-size verdict
/// ("workload exited unsuccessfully; inspect the redacted command output"),
/// which sent the operator hunting per host. The worker names its steps in
/// that log, so its tail is the diagnosis; it travels in the CLI error and,
/// through the platform failure field, into the persisted run object the
/// dashboard serves.
pub(crate) async fn job_output_tail(store: &JobStorage, job_id: &str) -> String {
    let path = format!("status/{job_id}/output/command_output.log");
    match store.read_bytes(&path).await {
        Ok(Some(bytes)) => {
            let text = String::from_utf8_lossy(&bytes);
            let lines: Vec<&str> = text.lines().collect();
            let tail = &lines[lines.len().saturating_sub(15)..];
            format!("; the job's last output:\n{}", tail.join("\n"))
        }
        Ok(None) => "; the job left no output log".to_string(),
        Err(error) => format!("; the job's output log could not be read: {error}"),
    }
}

/// Read what every submitted platform's job did, and decide where the build
/// stands. Nothing is enqueued here: a status read that started a build
/// would spend the fleet's build budget on a question.
///
/// A job the queue calls terminal is judged by its receipt, not by its exit
/// alone: the receipt must name this build, this job, this builder and this
/// source, say `passed`, and name the archive. Anything less is a failed
/// platform with the reason written down.
pub(crate) async fn refresh_build(
    store: &JobStorage,
    build: &mut BuildRun,
    m: &ReleasePipelineManifest,
) -> Result<(), CmdError> {
    for (name, platform) in build.platforms.iter_mut() {
        if platform.state != PlatformRunState::Submitted {
            continue;
        }
        let job_id = platform.job_id.clone();
        let prefix = format!("status/{job_id}/output/");
        // The queue's run reaper retires a settled job's record on its own
        // cadence, and a status read on another host's cadence may come
        // after it. The job's receipt outlives the record (the reaper keeps
        // a release job's output), and it is what every publish verifies, so
        // a job with no record and a receipt is judged by the receipt. A job
        // with neither is still queued or running.
        match read_terminal_job(store, &job_id).await? {
            Some(job) if matches!(job.state.as_str(), job_state::FAILED | job_state::CANCELLED) => {
                platform.state = PlatformRunState::Failed;
                platform.failure = Some(format!(
                    "build job {job_id} ended {}{}",
                    job.state,
                    job_output_tail(store, &job_id).await
                ));
                continue;
            }
            Some(_) => {}
            None => {
                if store
                    .read_bytes(&format!("{prefix}receipt.json"))
                    .await?
                    .is_none()
                {
                    continue;
                }
            }
        }
        let receipt = match store.read_bytes(&format!("{prefix}receipt.json")).await? {
            Some(bytes) => serde_json::from_slice::<BuildReceipt>(&bytes).map_err(|error| {
                CmdError::click(format!(
                    "build job {job_id} wrote an unreadable receipt: {error}"
                ))
            })?,
            None => {
                platform.state = PlatformRunState::Failed;
                platform.failure = Some(format!("build job {job_id} omitted receipt"));
                continue;
            }
        };
        if receipt.run_id != build.build_id
            || receipt.job_id != job_id
            || receipt.product != build.product
            || receipt.version != build.version
            || receipt.platform != *name
            || receipt.builder != platform.builder
            || receipt.source_commit != build.source_commit
            || receipt.source_sha256 != build.source_sha256
            || receipt.manifest_sha256 != build.manifest_sha256
        {
            platform.state = PlatformRunState::Failed;
            platform.failure = Some(format!(
                "build job {job_id} returned a receipt for another build"
            ));
            continue;
        }
        if receipt.status != StepStatus::Passed {
            platform.state = PlatformRunState::Failed;
            platform.failure = Some(
                receipt
                    .failure
                    .unwrap_or_else(|| format!("build job {job_id} did not pass")),
            );
            continue;
        }
        let Some(artifact) = receipt.artifact else {
            platform.state = PlatformRunState::Failed;
            platform.failure = Some(format!("build job {job_id} omitted archive"));
            continue;
        };
        platform.state = PlatformRunState::Qualified;
        platform.artifact_sha256 = Some(artifact.sha256);
        platform.failure = None;
    }
    build.state = build_state(build, m);
    Ok(())
}

/// `passed` once every required platform passed and no platform is still
/// building; `failed` as soon as a required platform failed; `waiting`
/// otherwise. An optional platform's failure is recorded on the platform
/// and does not fail the build, as it does not fail a release.
fn build_state(build: &BuildRun, m: &ReleasePipelineManifest) -> BuildRunState {
    let mut waiting = false;
    for (name, recipe) in &m.platforms {
        match build.platforms.get(name).map(|platform| &platform.state) {
            Some(PlatformRunState::Failed) if recipe.required => return BuildRunState::Failed,
            Some(PlatformRunState::Qualified | PlatformRunState::Failed) => {}
            None if recipe.required && build.state == BuildRunState::Failed => {
                return BuildRunState::Failed;
            }
            _ => waiting = true,
        }
    }
    if waiting {
        BuildRunState::Waiting
    } else {
        BuildRunState::Passed
    }
}
