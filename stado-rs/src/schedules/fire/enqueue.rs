//! Turning one claimed occurrence into a queued job: the overlap check the
//! skip policy consults, and the submission that carries a reservation from
//! `claimed` through `enqueuing` to an accepted job — or retires it.

use chrono::{DateTime, Utc};

use crate::queue::submit::{submit_batch, SubmitOptions};
use crate::queue::{JobStorage, StorageError};
use crate::schedules::{
    abandon_pending_occurrence, accept_pending_occurrence, begin_pending_occurrence,
    release_pending_occurrence, Schedule,
};

/// What this schedule's most recent fire is doing while it is still running
/// or queued — where it is, since when, until when its worker's lease holds
/// and what put it back in the queue — or `None` once it has left both. Two
/// direct reads by id — cheap, unlike scanning queue/ (14k+ blobs).
pub(super) async fn prev_instance_live(
    store: &JobStorage,
    sched: &Schedule,
    now: DateTime<Utc>,
) -> Option<String> {
    if sched.last_job_id.is_empty() {
        return None;
    }
    let id = &sched.last_job_id;
    let live = async {
        if let Some(job) = store.read_job("running", id).await? {
            return Ok::<_, StorageError>(Some(describe("running", &job, now)));
        }
        Ok(store
            .read_job("queue", id)
            .await?
            .map(|job| describe("queued", &job, now)))
    };
    match live.await {
        Ok(found) => found,
        // A read error is not proof the prior instance is gone; be
        // conservative and treat it as live so overlap_policy=skip holds.
        Err(error) => Some(format!(
            "of unknown state: its record could not be read ({error}), so it is treated as live"
        )),
    }
}

/// One live job as the skip line names it: its place, when it started and
/// how long ago, its lease, and the reason a reaper stored when it put the job
/// back.
fn describe(place: &str, job: &crate::models::Job, now: DateTime<Utc>) -> String {
    let mut said = place.to_string();
    if let Some(started) = job.started_at.as_deref() {
        said.push_str(&format!(", started {started}"));
        if let Some(at) = crate::schedules::parse_iso(started) {
            said.push_str(&format!(" ({} s ago)", (now - at).num_seconds()));
        }
    }
    if let Some(lease) = job.lease_expires_at.as_deref() {
        said.push_str(&format!(", worker lease until {lease}"));
    }
    if let Some(host) = job.instance_ref.as_deref() {
        said.push_str(&format!(", on {host}"));
    }
    if let Some(error) = job.error.as_deref() {
        said.push_str(&format!(
            ", put back after: {error} (restarts {})",
            job.restarts
        ));
    }
    said
}

pub(super) async fn enqueue_pending_occurrence(
    store: &JobStorage,
    sched: Schedule,
    owner: &str,
    fired_at: DateTime<Utc>,
) -> Result<Option<crate::models::Job>, StorageError> {
    let pending = sched.pending_occurrence.clone().ok_or_else(|| {
        StorageError::Other(format!(
            "schedule {} has no pending occurrence",
            sched.schedule_id
        ))
    })?;
    let Some(sched) =
        begin_pending_occurrence(store, &sched.schedule_id, &pending.occurrence_key, owner).await?
    else {
        return Ok(None);
    };
    let pending = sched
        .pending_occurrence
        .clone()
        .expect("begin preserves pending occurrence");
    let options = SubmitOptions {
        bucket: store.bucket_name().to_string(),
        run_id: pending.run_id.clone(),
        schedule_id: sched.schedule_id.clone(),
        ..sched.submit_options()
    };
    let commands = [sched.command.clone()];
    let mut jobs = match submit_batch(&commands, &options).await {
        Ok(jobs) => jobs,
        Err(error) => {
            // A validation rejection is a property of this exact request, so
            // retrying it forever would pin the reservation and silently
            // retire the schedule. Drop the occurrence and let the cadence
            // continue; only transient failures stay recoverable. A build
            // budget that refused the compile rejects it the same way; one
            // that could not be read or written is an outage, retried.
            let rejected = match &error {
                crate::queue::submit::SubmitError::Validation(_) => true,
                crate::queue::submit::SubmitError::Charge(charge) => {
                    charge.failure == Some(crate::primitives::failure::FailureCode::Refused)
                }
                _ => false,
            };
            if rejected {
                abandon_pending_occurrence(
                    store,
                    &sched.schedule_id,
                    &pending.occurrence_key,
                    owner,
                )
                .await?;
            } else {
                release_pending_occurrence(
                    store,
                    &sched.schedule_id,
                    &pending.occurrence_key,
                    owner,
                )
                .await;
            }
            return Err(StorageError::Other(format!(
                "durable schedule enqueue {}: {error}",
                if rejected {
                    "rejected this occurrence"
                } else {
                    "failed"
                }
            )));
        }
    };
    let job = jobs
        .pop()
        .ok_or_else(|| StorageError::Other("durable schedule enqueue returned no job".into()))?;
    if !accept_pending_occurrence(
        store,
        &sched.schedule_id,
        &pending.occurrence_key,
        owner,
        &job,
        fired_at,
    )
    .await?
    {
        return Err(StorageError::StorageConflict(format!(
            "schedule {} ownership changed after durable enqueue",
            sched.schedule_id
        )));
    }
    Ok(Some(job))
}
