//! `stado cancel JOB_ID [--terminate]` performs one durable, idempotent
//! cancellation transition, and `stado cancel --queued` performs it for every
//! job still waiting in the queue. A cancellation of a named job writes the
//! marker consumed by agents/coordinators and moves the job to `cancelled/`;
//! `--queued` moves only jobs still unclaimed. A cancelled job is never
//! deleted or mislabeled as failed.
//!
//! Agent execution is cancelled without deleting its shared worker VM.
//! Dedicated Box resources require a matching provider lease before deletion.
//! Terminal job references never authorize a new provider delete.
//! `--terminate` additionally reports the instance lookup and fails loudly
//! when a running job has no recoverable instance record.
//!
//! `--queued` exists because emptying a queue one id at a time is work nobody
//! finishes: a fleet holding dozens of queued jobs nobody wants otherwise
//! has only one route, dozens of commands.

use crate::machine::{
    capture_cancellation_allocation, fence_cancellation, request_provider_removal, utcnow,
};
use crate::models::job_state;
use crate::queue::runs;
use crate::queue::submit::default_store;
use crate::queue::JobStorage;

use crate::cli::CmdError;

/// What `--terminate` did about the job's cloud instance.
enum Termination {
    /// The provider accepted deletion; status must still observe removal.
    DeletionAccepted {
        instance_ref: String,
        source: String,
    },
    /// The reference names an agent slot, not a job-owned VM.
    Agent { instance_ref: String },
    /// Neither the job document nor the provider lease names an instance.
    /// `expected` is true where that is the correct state — a job still in
    /// `queue/` has not reached a provider, and a terminal job's agent
    /// cleared the reference on the way out. It is false only for a job in
    /// `running/`, which by definition should be holding something.
    NoRecord { expected: bool },
}

pub async fn run(job_id: Option<&str>, queued: bool, terminate: bool) -> Result<(), CmdError> {
    let store = default_store(crate::config::bucket()).await?;
    match (job_id, queued) {
        (Some(job_id), false) => cancel_one(&store, job_id, terminate).await,
        (None, true) => cancel_queue(&store).await,
        (Some(_), true) => Err(CmdError::usage(
            "cancel takes a job id or --queued, not both: --queued already names every job \
             waiting in the queue",
        )),
        (None, false) => Err(CmdError::usage(
            "cancel needs a job id, or --queued for every job still waiting in the queue",
        )),
    }
}

/// Cancel every job the queue still holds unclaimed.
///
/// Only `queue/` is read. `queue_priority/` beside it is the ordering index —
/// `<inv_priority>-<created_at>-<job_id>.json` markers plus a migration
/// sentinel — and reading those names as job ids is how the first pass
/// reported `Job .migration not found`. A marker goes when the job it points
/// at does.
///
/// Read first, then cancel each while it is still unclaimed: the move out of
/// `queue/` is fenced on the generation read, so a job a host claims between
/// the listing and its turn is left running and counted apart, never
/// followed into `running/`. A queued job holds no provider capacity, so
/// there is nothing to terminate and `--terminate` changes nothing here.
async fn cancel_queue(store: &JobStorage) -> Result<(), CmdError> {
    let mut cancelled = 0usize;
    let mut claimed = 0usize;
    let mut failed: Vec<String> = Vec::new();
    for job_id in store.list_job_ids("queue").await? {
        match cancel_queued_in_store(store, &job_id).await {
            Ok(true) => cancelled += 1,
            Ok(false) => claimed += 1,
            Err(error) => failed.push(format!("{job_id}: {error}")),
        }
    }
    println!(
        "cancelled {cancelled} queued job(s); {claimed} were claimed by a host first \
         and keep running"
    );
    if failed.is_empty() {
        return Ok(());
    }
    Err(CmdError::click(format!(
        "{} queued job(s) could not be cancelled:\n  {}",
        failed.len(),
        failed.join("\n  ")
    ))
    .stating(crate::primitives::failure::FailureCode::InfraDown))
}

async fn cancel_one(store: &JobStorage, job_id: &str, terminate: bool) -> Result<(), CmdError> {
    let job = fence_cancellation(store, job_id).await.map_err(|error| {
        CmdError::click(format!(
            "cancel {job_id} [{}]: {}",
            error.code, error.message
        ))
        .stating(error.failure_code())
    })?;
    let terminated = terminate_instance(store, &job).await?;
    if terminate {
        report(&terminated, job_id);
    }
    if job_state::is_terminal(&job.state) {
        println!("Job {job_id} is already terminal ({}); historical references did not authorize deletion", job.state);
        return Ok(());
    }
    cancel_after_fence(store, job_id).await?;

    if terminate && matches!(terminated, Termination::NoRecord { expected: false }) {
        return Err(CmdError::click(format!(
            "--terminate found nothing to delete for {job_id}: it was running but neither \
             the job document nor provider lease records an instance. The durable cancellation \
             remains visible; inspect the provider inventory for orphaned capacity."
        ))
        .stating(crate::primitives::failure::FailureCode::NotFound));
    }
    Ok(())
}

/// Request removal using the allocation retained in the durable fence.
async fn terminate_instance(
    store: &JobStorage,
    job: &crate::models::Job,
) -> Result<Termination, CmdError> {
    let job_id = job.job_id.as_str();
    let recorded = request_provider_removal(store, job).await.map_err(|exc| {
        CmdError::click(format!("request provider removal for {job_id}: {exc}"))
            .stating(crate::primitives::failure::FailureCode::InfraDown)
    })?;
    let Some(instance) = recorded else {
        let running = job.state == job_state::RUNNING;
        return Ok(Termination::NoRecord { expected: !running });
    };
    if instance.agent {
        return Ok(Termination::Agent {
            instance_ref: instance.instance_ref,
        });
    }
    Ok(Termination::DeletionAccepted {
        instance_ref: instance.instance_ref,
        source: instance.source,
    })
}

/// Say what happened to the instance, including when nothing did.
fn report(outcome: &Termination, job_id: &str) {
    match outcome {
        Termination::DeletionAccepted {
            instance_ref,
            source,
        } => {
            println!("Requested deletion of instance {instance_ref} (recorded in {source}); machine status observes removal");
        }
        Termination::Agent { instance_ref } => {
            println!(
                "{instance_ref} is agent execution; cancellation stops its workload, \
                 not the shared worker VM"
            );
        }
        Termination::NoRecord { expected: true } => {
            println!("No cloud instance recorded for {job_id} — nothing to delete");
        }
        Termination::NoRecord { expected: false } => {
            println!(
                "NOTHING DELETED: {job_id} is running but no instance reference is recorded \
                 in its job document or in provider-leases/{job_id}.json"
            );
        }
    }
}

async fn cancel_after_fence(store: &JobStorage, job_id: &str) -> Result<(), CmdError> {
    // Cancelled first: cancelling twice is the common retry, and the queue's
    // own terminal set is what "already terminal" means.
    for prefix in [
        runs::CANCELLED,
        runs::COMPLETED,
        runs::UPLOADED,
        runs::FAILED,
    ] {
        if let Some(job) = store.read_job(prefix, job_id).await? {
            println!("Job {job_id} is already terminal ({})", job.state);
            return Ok(());
        }
    }

    if let Some(mut job) = store.read_job("queue", job_id).await? {
        capture_cancellation_allocation(store, &job)
            .await
            .map_err(|error| {
                CmdError::click(format!(
                    "cancel {job_id} [{}]: {}",
                    error.code, error.message
                ))
                .stating(error.failure_code())
            })?;
        job.state = job_state::CANCELLED.into();
        job.completed_at = Some(utcnow());
        job.error = Some("cancelled".into());
        match store.move_job(&job, "queue", "cancelled").await {
            Ok(()) => {
                println!("Cancelled {job_id}");
                return Ok(());
            }
            Err(crate::queue::StorageError::StorageConflict(_)) => {}
            Err(error) => return Err(error.into()),
        }
    }

    if let Some(mut job) = store.read_job("running", job_id).await? {
        capture_cancellation_allocation(store, &job)
            .await
            .map_err(|error| {
                CmdError::click(format!(
                    "cancel {job_id} [{}]: {}",
                    error.code, error.message
                ))
                .stating(error.failure_code())
            })?;
        job.state = job_state::CANCELLED.into();
        job.completed_at = Some(utcnow());
        job.error = Some("cancelled".into());
        job.instance_ref = None;
        store.move_job(&job, "running", "cancelled").await?;
        println!("Cancelled {job_id}");
        return Ok(());
    }

    Err(CmdError::refused(format!("Job {job_id} not found")))
}

/// Cancel `job_id` only while no host has claimed it, for callers that decided
/// from a queued read (a waiting release, a superseded run, a silent pinned
/// host). The move out of `queue/` is fenced on the generation just read, so a
/// claim that lands in between makes it lose instead of following the job
/// into `running/`, and no cancellation marker is written, because the
/// coordinator reaps a running job that has one. `stado cancel` follows a
/// claimed job on purpose, for an operator who asked to stop it; these
/// callers never did, and following the claim there would cancel a build
/// while its agent writes the result. Answers whether the job was cancelled.
pub(crate) async fn cancel_queued_in_store(
    store: &JobStorage,
    job_id: &str,
) -> Result<bool, CmdError> {
    let Some(mut job) = store.read_job("queue", job_id).await? else {
        return Ok(false);
    };
    job.state = job_state::CANCELLED.into();
    job.completed_at = Some(utcnow());
    job.error = Some("cancelled".into());
    match store.move_job(&job, "queue", "cancelled").await {
        Ok(()) => Ok(true),
        Err(crate::queue::StorageError::StorageConflict(_)) => Ok(false),
        Err(error) => Err(error.into()),
    }
}
