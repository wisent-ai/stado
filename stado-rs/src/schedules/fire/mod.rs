//! Dispatch (Python `schedules/fire.py`): the coordinator sweep that fires
//! every due, enabled schedule once, and the operator's manual fire, both
//! going through the same durable occurrence reservation.
//!
//! [`enqueue`] carries one claimed occurrence into the queue and settles it.

use chrono::{DateTime, Utc};

use crate::queue::submit::stable_run_id;
use crate::queue::{JobStorage, StorageError};
use crate::schedules::{
    advance_due_without_work, compute_next_due, list_schedule_ids, parse_iso, read_schedule,
    reserve_due_occurrence, reserve_manual_occurrence, takeover_pending_occurrence,
};

mod enqueue;

use enqueue::{enqueue_pending_occurrence, prev_instance_live};

/// Fire every due+enabled schedule once. Returns the number fired.
pub async fn fire_due_schedules(
    store: &JobStorage,
    mut log: impl FnMut(&str),
    now: DateTime<Utc>,
) -> Result<i64, StorageError> {
    let mut fired = 0;
    for schedule_id in list_schedule_ids(store).await? {
        let Some(sched) = read_schedule(store, &schedule_id).await? else {
            continue;
        };
        let owner = uuid::Uuid::new_v4().simple().to_string();
        if sched.pending_occurrence.is_some() {
            let Some(claimed) = takeover_pending_occurrence(store, &schedule_id, &owner).await?
            else {
                log(&format!(
                    "schedule {schedule_id}: pending occurrence is leased by another coordinator"
                ));
                continue;
            };
            match enqueue_pending_occurrence(store, claimed, &owner, now).await {
                Ok(Some(job)) => {
                    fired += 1;
                    log(&format!(
                        "schedule {schedule_id}: recovered durable occurrence as job {} (run {})",
                        job.job_id, job.run_id
                    ));
                }
                Ok(None) => log(&format!(
                    "schedule {schedule_id}: lost pending occurrence ownership"
                )),
                Err(error) => log(&format!(
                    "schedule {schedule_id}: pending occurrence enqueue failed: {error}"
                )),
            }
            continue;
        }
        if !sched.enabled || sched.next_due_at.is_empty() {
            continue;
        }
        let occurrence_at = sched.next_due_at.clone();
        let Some(due) = parse_iso(&occurrence_at) else {
            log(&format!(
                "schedule {schedule_id}: unparseable next_due_at={occurrence_at:?}; skipping"
            ));
            continue;
        };
        if due > now {
            continue;
        }
        let next_due = crate::models::isoformat_utc(
            compute_next_due(&sched.cron, now, &sched.tz)
                .map_err(|error| StorageError::Other(error.to_string()))?,
        );
        if sched.overlap_policy == "skip" && prev_instance_live(store, &sched).await {
            if advance_due_without_work(store, &schedule_id, &occurrence_at, &next_due).await? {
                log(&format!(
                    "schedule {schedule_id}: skip fire (prior job {} still live)",
                    sched.last_job_id
                ));
            }
            continue;
        }
        let Some(claimed) =
            reserve_due_occurrence(store, &schedule_id, &occurrence_at, &next_due, &owner).await?
        else {
            log(&format!(
                "schedule {schedule_id}: lost occurrence reservation race"
            ));
            continue;
        };
        match enqueue_pending_occurrence(store, claimed, &owner, now).await {
            Ok(Some(job)) => {
                fired += 1;
                log(&format!(
                    "schedule {schedule_id}: fired job {} (run {}); next_due={next_due}",
                    job.job_id, job.run_id
                ));
            }
            Ok(None) => log(&format!(
                "schedule {schedule_id}: lost occurrence ownership before enqueue"
            )),
            Err(error) => log(&format!(
                "schedule {schedule_id}: occurrence enqueue failed and remains recoverable: {error}"
            )),
        }
    }
    Ok(fired)
}

/// Manually fire a schedule through the same durable occurrence reservation as
/// the coordinator. A pending crash recovery is completed before a new manual
/// occurrence can be created.
pub async fn fire_schedule_now(
    store: &JobStorage,
    schedule_id: &str,
    retry_token: &str,
    now: DateTime<Utc>,
) -> Result<Option<crate::models::Job>, StorageError> {
    let token = format!("{schedule_id}\0{retry_token}");
    let run_id = stable_run_id("schedule", &token);
    if let Some(manifest) = crate::queue::runs::read_run(store, &run_id).await? {
        crate::queue::submit::validate_stored_run_manifest(
            &serde_json::Value::Object(manifest.clone()),
            &run_id,
        )
        .map_err(|error| StorageError::Other(error.to_string()))?;
        // Only an entry the queue already admitted answers a repeat. A run
        // manifest that stopped at planned/claimed/enqueuing has no job yet:
        // replaying its planned document would report a fire that never
        // reached the queue, so that retry has to finish the submission.
        if let Some(entry) = manifest
            .get("entries")
            .and_then(serde_json::Value::as_array)
            .and_then(|entries| entries.first())
        {
            let admitted = matches!(
                entry.get("state").and_then(serde_json::Value::as_str),
                Some("accepted" | "terminal" | "reaped")
            );
            let retained = entry
                .get("outcome")
                .and_then(|outcome| outcome.get("job"))
                .or_else(|| entry.get("planned_job"))
                .cloned();
            if let (true, Some(job)) = (admitted, retained) {
                return serde_json::from_value(job)
                    .map(Some)
                    .map_err(StorageError::Json);
            }
        }
    }
    // A pending occurrence that is not this token's is somebody else's work —
    // a crashed coordinator fire, typically. Finish it first, then reserve and
    // enqueue the occurrence this token names: returning that unrelated job as
    // the manual fire would report work the operator asked for as done while
    // it was never submitted.
    for _ in 0..2 {
        let Some(sched) = read_schedule(store, schedule_id).await? else {
            return Ok(None);
        };
        if sched.deleted && sched.pending_occurrence.is_none() {
            return Ok(None);
        }
        let owner = uuid::Uuid::new_v4().simple().to_string();
        match sched.pending_occurrence.as_ref() {
            Some(pending) if pending.run_id != run_id => {
                let Some(claimed) = takeover_pending_occurrence(store, schedule_id, &owner).await?
                else {
                    return Ok(None);
                };
                enqueue_pending_occurrence(store, claimed, &owner, now).await?;
                continue;
            }
            Some(_) => {
                let Some(claimed) = takeover_pending_occurrence(store, schedule_id, &owner).await?
                else {
                    return Ok(None);
                };
                return enqueue_pending_occurrence(store, claimed, &owner, now).await;
            }
            None => {
                let Some(claimed) =
                    reserve_manual_occurrence(store, schedule_id, &owner, retry_token).await?
                else {
                    return Ok(None);
                };
                return enqueue_pending_occurrence(store, claimed, &owner, now).await;
            }
        }
    }
    Ok(None)
}
