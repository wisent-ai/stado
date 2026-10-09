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

/// How a release job ended, from whichever record survives: its terminal
/// queue record; the receipt its worker wrote with its exit (every publish
/// verifies it in full before a byte is published); the outcome the queue's
/// run reaper retained in the job's run manifest when it retired the
/// record — a cancelled job, or one that failed before its worker wrote a
/// receipt, has no other witness; and last the job's transition record,
/// which names the terminal prefix a job reaped before its run retained it
/// moved into. `None` when the job has not ended by any of them.
///
/// Every reader of an ended release job comes here — the finishing pass,
/// publishing, the build's platform refresh and `stado release status` —
/// so a record one of them had not thought of cannot leave a run waiting on
/// a job that is over.
pub(crate) async fn settled(store: &JobStorage, id: &str) -> Result<Option<Job>, CmdError> {
    if let Some(job) = read_terminal_job(store, id).await? {
        return Ok(Some(job));
    }
    if let Some(bytes) = store
        .read_bytes(&format!("status/{id}/output/receipt.json"))
        .await?
    {
        let receipt: BuildReceipt = serde_json::from_slice(&bytes)?;
        let (state, error) = if receipt.status == StepStatus::Passed {
            (job_state::COMPLETED, None)
        } else {
            (job_state::FAILED, receipt.failure)
        };
        return Ok(Some(Job {
            job_id: id.to_string(),
            pinned_host: receipt.builder,
            state: state.to_string(),
            error,
            ..Job::default()
        }));
    }
    reaped(store, id).await
}

/// How a job whose queue record and receipt are both gone ended: the
/// outcome the queue's run reaper retained in the job's run manifest,
/// found through the index retention writes
/// ([`crate::queue::runs::retained_job`]), or the job's transition record,
/// which names the terminal prefix a job reaped before its run retained it
/// moved into. `None` when neither names the job.
pub(crate) async fn reaped(store: &JobStorage, id: &str) -> Result<Option<Job>, CmdError> {
    let retained = crate::queue::runs::retained_job(store, id)
        .await
        .map_err(|error| {
            CmdError::from(error).within(format!("read the reaped outcome of {id}"))
        })?;
    if let Some(job) = retained {
        return Ok(Some(job));
    }
    let Some(state) = store.ended_state(id).await.map_err(|error| {
        CmdError::from(error).within(format!("read the last transition of {id}"))
    })?
    else {
        return Ok(None);
    };
    Ok(Some(Job {
        job_id: id.to_string(),
        state,
        ..Job::default()
    }))
}

/// The release job's terminal record ([`settled`]). A job that is still
/// queued or running is an error naming where it is and, when queued, the
/// host it is pinned to.
pub(crate) async fn terminal(store: &JobStorage, id: &str) -> Result<Job, CmdError> {
    match ended(store, id).await? {
        Ended::Job(job) => Ok(*job),
        Ended::Queued { state, host } => Err(CmdError::click(format!(
            "release job {id} is still queued ({state}) on {host}; no host has claimed it. The \
             host's own decline is in its agent log: read it with `stado service logs <unit> \
             --host <host>`"
        ))
        .stating(crate::primitives::failure::FailureCode::InfraDown)),
        Ended::Running => Err(
            CmdError::click(format!("release job {id} is still running"))
                .stating(crate::primitives::failure::FailureCode::InfraDown),
        ),
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

/// The release job as it ended ([`settled`]), or where it still is. A
/// delivery pass that finds a job queued or running leaves the run
/// delivering and reads it again on a later pass; nothing here waits.
///
/// The settled records are read before the queue: only a worker that ran
/// this job writes its receipt, and a claimed job's queue record can outlive
/// the claim, so a job that finished with its receipt written was answered
/// "still queued" while `stado build status` called the same platform
/// passed.
pub(crate) async fn ended(store: &JobStorage, id: &str) -> Result<Ended, CmdError> {
    if let Some(job) = settled(store, id).await? {
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
    Err(CmdError::click(format!(
        "release job {id} has not reached a terminal state: no queue record, no receipt, no \
         reaped run retains it and no transition record names how it ended"
    ))
    .stating(crate::primitives::failure::FailureCode::InfraDown))
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
        // that ended without one — cancelled, or failed before its worker
        // wrote it — is read back from the outcome the reaper kept in its
        // submission's run manifest; without that read it stayed `building`
        // for as long as anyone asked. A job with neither is still queued
        // or running.
        let recorded = read_terminal_job(store, &job_id).await?;
        let receipt_bytes = store.read_bytes(&format!("{prefix}receipt.json")).await?;
        let found = match (recorded, &receipt_bytes) {
            (Some(job), _) => Some(job),
            (None, Some(_)) => None,
            (None, None) => match reaped(store, &job_id).await? {
                Some(job) => Some(job),
                None => continue,
            },
        };
        if let Some(job) = found
            .as_ref()
            .filter(|job| matches!(job.state.as_str(), job_state::FAILED | job_state::CANCELLED))
        {
            platform.state = PlatformRunState::Failed;
            platform.failure = Some(format!(
                "build job {job_id} ended {}{}",
                job.state,
                job_output_tail(store, &job_id).await
            ));
            continue;
        }
        let receipt = match receipt_bytes {
            Some(bytes) => serde_json::from_slice::<BuildReceipt>(&bytes).map_err(|error| {
                CmdError::click(format!(
                    "build job {job_id} wrote an unreadable receipt: {error}"
                ))
                .stating(crate::primitives::failure::FailureCode::InfraDown)
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
