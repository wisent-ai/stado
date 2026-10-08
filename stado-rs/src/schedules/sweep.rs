//! The coordinator's last schedule sweep, recorded in the store by every tick
//! that evaluates schedules, so a schedule still waiting past its next run can
//! say why: no coordinator sweeps the store this command reads, the last sweep
//! came before the occurrence fell due (the coordinator has not ticked since),
//! the sweep failed, or an occurrence is reserved and not yet enqueued.
//!
//! The record lives outside the `schedules/` prefix, which holds schedule
//! documents only.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::queue::{JobStorage, StorageError};

use super::{parse_iso, Schedule};

/// Where the last sweep is recorded in the coordinator's store.
pub const SWEEP_PATH: &str = "system/schedule-sweep.json";

/// One coordinator tick's schedule sweep.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Sweep {
    /// When the sweep evaluated the schedules, ISO-8601 UTC.
    pub at: String,
    /// The host whose coordinator swept.
    pub host: String,
    /// Occurrences it fired; absent when the sweep failed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fired: Option<i64>,
    /// The store error that stopped the sweep, when one did.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Overwrite the last-sweep record.
pub async fn record_sweep(store: &JobStorage, sweep: &Sweep) -> Result<(), StorageError> {
    let text = serde_json::to_string_pretty(sweep).map_err(|error| StorageError::Other(error.to_string()))?;
    store.upload_text(SWEEP_PATH, &text).await
}

/// The last recorded sweep, `None` when no coordinator ever recorded one in
/// this store.
pub async fn last_sweep(store: &JobStorage) -> Result<Option<Sweep>, StorageError> {
    let Some(text) = store.download_text(SWEEP_PATH).await? else {
        return Ok(None);
    };
    serde_json::from_str(&text)
        .map(Some)
        .map_err(|error| StorageError::Other(format!("{SWEEP_PATH} is unreadable: {error}")))
}

/// Why `schedule` has not fired the occurrence that fell due at its
/// `next_due_at`, or `None` while it is not yet due (or is paused, deleted or
/// carries no next run).
pub fn overdue(schedule: &Schedule, sweep: Option<&Sweep>, now: DateTime<Utc>) -> Option<String> {
    if !schedule.enabled || schedule.deleted || schedule.next_due_at.is_empty() {
        return None;
    }
    let due = parse_iso(&schedule.next_due_at)?;
    if due > now {
        return None;
    }
    if let Some(pending) = &schedule.pending_occurrence {
        return Some(format!(
            "occurrence {} is reserved by {} (state {}) and not yet enqueued; the next coordinator \
             tick takes it over and enqueues it",
            pending.occurrence_at, pending.owner, pending.state
        ));
    }
    let Some(sweep) = sweep else {
        return Some(format!(
            "it fell due at {} and no coordinator has recorded a schedule sweep in this store \
             ({SWEEP_PATH} is absent): no coordinator ticks against the store this command read, \
             so nothing fires its schedules",
            schedule.next_due_at
        ));
    };
    if let Some(error) = &sweep.error {
        return Some(format!(
            "the last schedule sweep, at {} on {}, failed: {error}",
            sweep.at, sweep.host
        ));
    }
    match parse_iso(&sweep.at) {
        Some(swept) if swept < due => Some(format!(
            "it fell due at {} and the last schedule sweep was at {} on {}: that coordinator has \
             not ticked since; its tick log on {} says what holds it",
            schedule.next_due_at, sweep.at, sweep.host, sweep.host
        )),
        Some(_) => Some(format!(
            "the schedule sweep at {} on {} ran after it fell due at {} and did not fire it; that \
             tick's log names the schedule",
            sweep.at, sweep.host, schedule.next_due_at
        )),
        None => Some(format!("{SWEEP_PATH} states an unreadable sweep time {:?}", sweep.at)),
    }
}
