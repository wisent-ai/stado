//! Retiring one occurrence once the queue has answered: hand the reservation
//! back for another coordinator, drop it when the request was rejected
//! outright, or record the accepted job against the schedule's bookkeeping.

use chrono::{DateTime, Utc};

use crate::queue::{JobStorage, StorageError};
use crate::schedules::Schedule;

use super::path;

pub(in crate::schedules) async fn release_pending_occurrence(
    store: &JobStorage,
    schedule_id: &str,
    occurrence_key: &str,
    owner: &str,
) {
    let path = path(schedule_id);
    let Ok(Some(versioned)) = store.read_text_versioned(&path).await else {
        return;
    };
    let Ok(mut sched) = Schedule::from_json(&versioned.content) else {
        return;
    };
    let Some(pending) = sched.pending_occurrence.as_mut() else {
        return;
    };
    if pending.occurrence_key != occurrence_key || pending.owner != owner {
        return;
    }
    pending.state = "claimed".into();
    pending.owner.clear();
    pending.lease_expires_at = Utc::now().to_rfc3339();
    let _ = store
        .compare_and_swap_text(&path, &versioned.version, &sched.to_json())
        .await;
}

/// Retire a reservation whose request the queue rejected outright, so the
/// schedule's later occurrences are not blocked behind an unsatisfiable one.
pub(in crate::schedules) async fn abandon_pending_occurrence(
    store: &JobStorage,
    schedule_id: &str,
    occurrence_key: &str,
    owner: &str,
) -> Result<(), StorageError> {
    let path = path(schedule_id);
    for _ in 0..16 {
        let Some(versioned) = store.read_text_versioned(&path).await? else {
            return Ok(());
        };
        let mut sched = Schedule::from_json(&versioned.content)?;
        let Some(pending) = sched.pending_occurrence.as_ref() else {
            return Ok(());
        };
        if pending.occurrence_key != occurrence_key || pending.owner != owner {
            return Ok(());
        }
        sched.pending_occurrence = None;
        match store
            .compare_and_swap_text(&path, &versioned.version, &sched.to_json())
            .await
        {
            Ok(_) => return Ok(()),
            Err(StorageError::StorageConflict(_)) => continue,
            Err(StorageError::NotFound(_)) => return Ok(()),
            Err(error) => return Err(error),
        }
    }
    Err(StorageError::StorageConflict(format!(
        "schedule {schedule_id} remained contended while abandoning occurrence"
    )))
}

pub(in crate::schedules) async fn accept_pending_occurrence(
    store: &JobStorage,
    schedule_id: &str,
    occurrence_key: &str,
    owner: &str,
    job: &crate::models::Job,
    fired_at: DateTime<Utc>,
) -> Result<bool, StorageError> {
    let path = path(schedule_id);
    for _ in 0..16 {
        let Some(versioned) = store.read_text_versioned(&path).await? else {
            return Ok(false);
        };
        let mut sched = Schedule::from_json(&versioned.content)?;
        let Some(pending) = sched.pending_occurrence.as_ref() else {
            return Ok(sched.last_run_id == job.run_id && sched.last_job_id == job.job_id);
        };
        if pending.occurrence_key != occurrence_key
            || pending.owner != owner
            || pending.state != "enqueuing"
            || pending.run_id != job.run_id
        {
            return Ok(false);
        }
        sched.last_fired_at = Some(crate::models::isoformat_utc(fired_at));
        sched.last_run_id = job.run_id.clone();
        sched.last_job_id = job.job_id.clone();
        sched.fire_count += 1;
        sched.pending_occurrence = None;
        match store
            .compare_and_swap_text(&path, &versioned.version, &sched.to_json())
            .await
        {
            Ok(_) => return Ok(true),
            Err(StorageError::StorageConflict(_)) => continue,
            Err(StorageError::NotFound(_)) => return Ok(false),
            Err(error) => return Err(error),
        }
    }
    Err(StorageError::StorageConflict(format!(
        "schedule {schedule_id} remained contended while accepting occurrence"
    )))
}
