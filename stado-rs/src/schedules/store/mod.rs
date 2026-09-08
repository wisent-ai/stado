//! Schedule persistence (Python `schedules/store.py`): the blob path, the
//! generation-pinned read, the listing, and the create/enable/delete
//! compare-and-swaps the CLI drives.
//!
//! The occurrence bookkeeping the coordinator drives is split off: [`claim`]
//! reserves one occurrence and advances `next_due_at` in the same swap, and
//! [`settle`] releases, abandons or accepts that reservation once the queue
//! has answered.

use crate::queue::{JobStorage, StorageError};

use super::{Schedule, PREFIX};

mod claim;
mod settle;

pub(in crate::schedules) use claim::{
    advance_due_without_work, begin_pending_occurrence, reserve_due_occurrence,
    reserve_manual_occurrence, takeover_pending_occurrence,
};
pub(in crate::schedules) use settle::{
    abandon_pending_occurrence, accept_pending_occurrence, release_pending_occurrence,
};

// ---------------------------------------------------------------------------
// store (Python schedules/store.py)
//
// Reuses JobStorage's prefix-agnostic blob helpers for the common paths.
// Every mutation is a compare-and-swap over a versioned read: the occurrence
// reservation and next_due_at advance in one swap, so two overlapping
// coordinator invocations can never double-fire the same occurrence.
// ---------------------------------------------------------------------------

fn path(schedule_id: &str) -> String {
    format!("{PREFIX}/{schedule_id}.json")
}

/// Fresh, generation-pinned read of `path` (Python `_read_fresh_text`).
///
/// A plain no-generation download on the wisent-compute bucket can return
/// a stale (edge-cached) copy of an object that was just overwritten in
/// place — confirmed live 2026-06-01: a schedule's next_due_at update read
/// back as the OLD value via `store._download_text` even though the new
/// generation was already the latest. The existing queue never hit this
/// because it is write-once-then-delete; schedules overwrite the same blob
/// every tick (read-modify-write of next_due_at), which is exactly the
/// pattern the cache breaks. `read_text_versioned` fetches the current
/// generation first and pins the download to it, so the bytes are
/// guaranteed to be the latest. (Python falls back to a plain read on the
/// gsutil/Azure paths; our local backend's versioned read is a locked
/// content read, which is already fresh.)
async fn read_fresh_text(store: &JobStorage, path: &str) -> Result<Option<String>, StorageError> {
    Ok(store
        .read_text_versioned(path)
        .await?
        .map(|versioned| versioned.content))
}

/// Schedule ids of every `schedules/<id>.json` blob.
pub async fn list_schedule_ids(store: &JobStorage) -> Result<Vec<String>, StorageError> {
    let paths = store.list_paths(&format!("{PREFIX}/"), 0).await?;
    Ok(paths
        .iter()
        .filter_map(|name| name.rsplit('/').next())
        .filter(|base| base.ends_with(".json"))
        .map(|base| base[..base.len() - ".json".len()].to_string())
        .collect())
}

/// Read one schedule; `None` when it does not exist.
pub async fn read_schedule(
    store: &JobStorage,
    schedule_id: &str,
) -> Result<Option<Schedule>, StorageError> {
    let Some(data) = read_fresh_text(store, &path(schedule_id)).await? else {
        return Ok(None);
    };
    Ok(Some(Schedule::from_json(&data)?))
}

/// Every schedule, in listing order.
pub async fn list_schedules(store: &JobStorage) -> Result<Vec<Schedule>, StorageError> {
    let mut out = Vec::new();
    for schedule_id in list_schedule_ids(store).await? {
        if let Some(sched) = read_schedule(store, &schedule_id).await? {
            if !sched.deleted {
                out.push(sched);
            }
        }
    }
    Ok(out)
}

/// Create a schedule exactly once. Mutable bookkeeping/configuration uses
/// dedicated CAS updates so it cannot erase a pending occurrence reservation.
pub async fn write_schedule(store: &JobStorage, sched: &Schedule) -> Result<(), StorageError> {
    if store
        .create_text_if_absent(&path(&sched.schedule_id), &sched.to_json())
        .await?
    {
        Ok(())
    } else {
        Err(StorageError::StorageConflict(format!(
            "schedule {} already exists",
            sched.schedule_id
        )))
    }
}

/// CAS-update enablement while preserving any pending occurrence lease.
pub async fn set_schedule_enabled(
    store: &JobStorage,
    schedule_id: &str,
    enabled: bool,
    next_due_at: Option<&str>,
) -> Result<Option<Schedule>, StorageError> {
    let path = path(schedule_id);
    for _ in 0..16 {
        let Some(versioned) = store.read_text_versioned(&path).await? else {
            return Ok(None);
        };
        let mut sched = Schedule::from_json(&versioned.content)?;
        if sched.deleted {
            return Ok(None);
        }
        sched.enabled = enabled;
        if sched.pending_occurrence.is_none() {
            if let Some(next_due_at) = next_due_at {
                sched.next_due_at = next_due_at.to_string();
            }
        }
        match store
            .compare_and_swap_text(&path, &versioned.version, &sched.to_json())
            .await
        {
            Ok(_) => return Ok(Some(sched)),
            Err(StorageError::StorageConflict(_)) => continue,
            Err(StorageError::NotFound(_)) => return Ok(None),
            Err(error) => return Err(error),
        }
    }
    Err(StorageError::StorageConflict(format!(
        "schedule {schedule_id} remained contended during enablement update"
    )))
}

/// CAS-write a durable deletion tombstone; `false` when absent/already deleted.
pub async fn delete_schedule(store: &JobStorage, schedule_id: &str) -> Result<bool, StorageError> {
    let path = path(schedule_id);
    for _ in 0..16 {
        let Some(versioned) = store.read_text_versioned(&path).await? else {
            return Ok(false);
        };
        let mut sched = Schedule::from_json(&versioned.content)?;
        if sched.deleted {
            return Ok(false);
        }
        sched.deleted = true;
        sched.enabled = false;
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
        "schedule {schedule_id} remained contended during deletion"
    )))
}
