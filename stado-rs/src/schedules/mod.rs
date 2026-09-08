//! Recurring (cron) job schedules.
//!
//! Port of `stado/schedules/` (`model.py`, `store.py`, `fire.py`).
//!
//! A Schedule is a recurring job spec — NOT a Job. It is the frozen submit
//! payload (command + sizing/routing kwargs) plus a 5-field cron expression
//! and firing bookkeeping. The coordinator tick evaluates each enabled
//! schedule's `next_due_at` and submits a fresh job when it comes due.
//!
//! Stored at `<bucket>/schedules/<schedule_id>.json`, byte-compatible with
//! Python's `json.dumps(asdict(schedule), indent=2)`.
//!
//! The components are the seams the port already had: [`model`] is the
//! schedule record, its occurrence reservation and the JSON codec; [`cron`]
//! is the due-time arithmetic; [`store`] is the persistence, including the
//! compare-and-swaps that reserve and settle one occurrence; [`fire`] is the
//! dispatch, both the coordinator sweep and the manual fire. The blob prefix
//! and the id generator every component shares stay here. Every name a caller
//! outside this module uses is re-exported here, so `crate::schedules::<item>`
//! resolves exactly as before.
//!
//! # Cron engine deviation notes (croniter → `cron` crate)
//!
//! Python computes next-fire times with croniter; this port uses the `cron`
//! crate (6/7-field, seconds-first) behind a compat shim that:
//!   * expands a 5-field expression by prepending second `0`,
//!   * translates the day-of-week field from croniter numbering
//!     (0-6 or 0/7 = Sunday) to the crate's 1-7 (Sunday=1),
//!   * maps the croniter-only aliases `@annually`→`@yearly` and
//!     `@midnight`→`@daily` (the crate supports the other five natively),
//!   * replicates the Vixie-cron OR semantics croniter uses when BOTH
//!     day-of-month and day-of-week are restricted, by compiling two
//!     schedules (dom-only and dow-only) and taking the earliest fire —
//!     the crate only knows AND semantics.
//!
//! Corners where the crate CANNOT match croniter (documented, tested):
//!   * spring-forward nonexistent wall times: croniter maps "0 2 * * *" on
//!     the night 02:00 does not exist to 03:00 the same night; the crate
//!     skips to the next day. Autumn-transition ambiguous times agree (both
//!     pick the first occurrence).
//!   * croniter extensions the crate's parser lacks: `L` (last day of
//!     month) and wrap-around ranges such as `6-1` / `22-2`. These parse
//!     in croniter but fail here, so `cron_is_valid` returns false and the
//!     CLI refuses them at `schedule create` time.

mod cron;
mod fire;
mod model;
mod store;

pub use cron::{compute_next_due, cron_is_valid, CronError};
pub use fire::{fire_due_schedules, fire_schedule_now};
pub use model::{Schedule, ScheduleOccurrenceReservation};
pub use store::{
    delete_schedule, list_schedules, read_schedule, set_schedule_enabled, write_schedule,
};

// Shared inside the schedules tree only: each component imports what it needs
// from `crate::schedules` instead of reaching into a sibling component.
pub(in crate::schedules) use cron::parse_iso;
pub(in crate::schedules) use store::{
    abandon_pending_occurrence, accept_pending_occurrence, advance_due_without_work,
    begin_pending_occurrence, list_schedule_ids, release_pending_occurrence,
    reserve_due_occurrence, reserve_manual_occurrence, takeover_pending_occurrence,
};

/// Blob prefix holding schedule documents.
pub const PREFIX: &str = "schedules";

/// `sch-<8 hex>` — namespaced so a schedule id is never confused with a job
/// id in logs (Python `generate_schedule_id`).
pub fn generate_schedule_id() -> String {
    format!("sch-{}", hex::encode(&uuid::Uuid::new_v4().as_bytes()[..4]))
}
