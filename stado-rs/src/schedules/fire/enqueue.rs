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

/// True iff this schedule's most recent fire is still queued/running.
/// Two direct reads by id — cheap, unlike scanning queue/ (14k+ blobs).
pub(super) async fn prev_instance_live(store: &JobStorage, sched: &Schedule) -> bool {
    if sched.last_job_id.is_empty() {
        return false;
    }
    let live = async {
        Ok::<bool, StorageError>(
            store.read_job("queue", &sched.last_job_id).await?.is_some()
                || store
                    .read_job("running", &sched.last_job_id)
                    .await?
                    .is_some(),
        )
    };
    // A read error is not proof the prior instance is gone; be
    // conservative and treat it as live so overlap_policy=skip holds.
    live.await.unwrap_or(true)
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
            // continue; only transient failures stay recoverable.
            let rejected = matches!(error, crate::queue::submit::SubmitError::Validation(_));
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
