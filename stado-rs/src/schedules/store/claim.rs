//! Taking one occurrence: the compare-and-swap that reserves a due or manual
//! occurrence and advances `next_due_at` in the same swap, the takeover of a
//! reservation whose lease has lapsed, the transition into enqueuing, and the
//! due advance for a tick that deliberately does no work.

use chrono::{DateTime, Utc};

use crate::queue::submit::stable_run_id;
use crate::queue::{JobStorage, StorageError};
use crate::schedules::{Schedule, ScheduleOccurrenceReservation};

use super::path;

fn occurrence_token(schedule_id: &str, occurrence_at: &str) -> String {
    format!("{schedule_id}\0{occurrence_at}")
}

fn occurrence_lease_live(reservation: &ScheduleOccurrenceReservation) -> bool {
    DateTime::parse_from_rfc3339(&reservation.lease_expires_at)
        .ok()
        .is_some_and(|expires| expires > Utc::now())
}

pub(in crate::schedules) async fn reserve_due_occurrence(
    store: &JobStorage,
    schedule_id: &str,
    occurrence_at: &str,
    new_next_due_at: &str,
    owner: &str,
) -> Result<Option<Schedule>, StorageError> {
    let path = path(schedule_id);
    let Some(versioned) = store.read_text_versioned(&path).await? else {
        return Ok(None);
    };
    let mut sched = Schedule::from_json(&versioned.content)?;
    if sched.deleted {
        return Ok(None);
    }
    if sched.pending_occurrence.is_some() || sched.next_due_at != occurrence_at {
        return Ok(None);
    }
    let token = occurrence_token(schedule_id, occurrence_at);
    sched.next_due_at = new_next_due_at.to_string();
    sched.pending_occurrence = Some(ScheduleOccurrenceReservation {
        occurrence_key: stable_run_id("schedule-occurrence", &token),
        occurrence_at: occurrence_at.to_string(),
        run_id: stable_run_id("schedule", &token),
        state: "claimed".into(),
        owner: owner.to_string(),
        lease_expires_at: (Utc::now() + chrono::Duration::minutes(15)).to_rfc3339(),
    });
    match store
        .compare_and_swap_text(&path, &versioned.version, &sched.to_json())
        .await
    {
        Ok(_) => Ok(Some(sched)),
        Err(StorageError::StorageConflict(_) | StorageError::NotFound(_)) => Ok(None),
        Err(error) => Err(error),
    }
}

pub(in crate::schedules) async fn reserve_manual_occurrence(
    store: &JobStorage,
    schedule_id: &str,
    owner: &str,
    retry_token: &str,
) -> Result<Option<Schedule>, StorageError> {
    let path = path(schedule_id);
    let Some(versioned) = store.read_text_versioned(&path).await? else {
        return Ok(None);
    };
    let mut sched = Schedule::from_json(&versioned.content)?;
    if sched.deleted {
        return Ok(None);
    }
    if sched.pending_occurrence.is_some() {
        return Ok(None);
    }
    let token = format!("{schedule_id}\0{retry_token}");
    let occurrence_at = format!("manual:{}", stable_run_id("schedule-manual", &token));
    sched.pending_occurrence = Some(ScheduleOccurrenceReservation {
        occurrence_key: stable_run_id("schedule-occurrence", &token),
        occurrence_at,
        run_id: stable_run_id("schedule", &token),
        state: "claimed".into(),
        owner: owner.to_string(),
        lease_expires_at: (Utc::now() + chrono::Duration::minutes(15)).to_rfc3339(),
    });
    match store
        .compare_and_swap_text(&path, &versioned.version, &sched.to_json())
        .await
    {
        Ok(_) => Ok(Some(sched)),
        Err(StorageError::StorageConflict(_) | StorageError::NotFound(_)) => Ok(None),
        Err(error) => Err(error),
    }
}

pub(in crate::schedules) async fn takeover_pending_occurrence(
    store: &JobStorage,
    schedule_id: &str,
    owner: &str,
) -> Result<Option<Schedule>, StorageError> {
    let path = path(schedule_id);
    let Some(versioned) = store.read_text_versioned(&path).await? else {
        return Ok(None);
    };
    let mut sched = Schedule::from_json(&versioned.content)?;
    let Some(pending) = sched.pending_occurrence.as_mut() else {
        return Ok(None);
    };
    if occurrence_lease_live(pending) && pending.owner != owner {
        return Ok(None);
    }
    pending.state = "claimed".into();
    pending.owner = owner.to_string();
    pending.lease_expires_at = (Utc::now() + chrono::Duration::minutes(15)).to_rfc3339();
    match store
        .compare_and_swap_text(&path, &versioned.version, &sched.to_json())
        .await
    {
        Ok(_) => Ok(Some(sched)),
        Err(StorageError::StorageConflict(_) | StorageError::NotFound(_)) => Ok(None),
        Err(error) => Err(error),
    }
}

pub(in crate::schedules) async fn begin_pending_occurrence(
    store: &JobStorage,
    schedule_id: &str,
    occurrence_key: &str,
    owner: &str,
) -> Result<Option<Schedule>, StorageError> {
    let path = path(schedule_id);
    let Some(versioned) = store.read_text_versioned(&path).await? else {
        return Ok(None);
    };
    let mut sched = Schedule::from_json(&versioned.content)?;
    let Some(pending) = sched.pending_occurrence.as_mut() else {
        return Ok(None);
    };
    if pending.occurrence_key != occurrence_key
        || pending.owner != owner
        || pending.state != "claimed"
    {
        return Ok(None);
    }
    pending.state = "enqueuing".into();
    match store
        .compare_and_swap_text(&path, &versioned.version, &sched.to_json())
        .await
    {
        Ok(_) => Ok(Some(sched)),
        Err(StorageError::StorageConflict(_) | StorageError::NotFound(_)) => Ok(None),
        Err(error) => Err(error),
    }
}

pub(in crate::schedules) async fn advance_due_without_work(
    store: &JobStorage,
    schedule_id: &str,
    expected_due_at: &str,
    new_next_due_at: &str,
) -> Result<bool, StorageError> {
    let path = path(schedule_id);
    let Some(versioned) = store.read_text_versioned(&path).await? else {
        return Ok(false);
    };
    let mut sched = Schedule::from_json(&versioned.content)?;
    if sched.pending_occurrence.is_some() || sched.next_due_at != expected_due_at {
        return Ok(false);
    }
    sched.next_due_at = new_next_due_at.to_string();
    match store
        .compare_and_swap_text(&path, &versioned.version, &sched.to_json())
        .await
    {
        Ok(_) => Ok(true),
        Err(StorageError::StorageConflict(_) | StorageError::NotFound(_)) => Ok(false),
        Err(error) => Err(error),
    }
}
