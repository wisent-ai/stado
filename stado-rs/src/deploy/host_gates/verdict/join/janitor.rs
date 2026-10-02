//! Whether the host's disk janitor is healthy, held, or silent.
//!
//! A running workload can legitimately prevent a janitor pass. Persistent
//! prevention reports a held lock; absence of both progress and prevention
//! reports a stalled janitor. These states require different remedies.

use chrono::{DateTime, Utc};

use crate::deploy::host_disk;

/// What the two janitor gates read.
pub(super) struct JanitorHealth {
    /// A workload holds the run lock and every pass is being turned away.
    pub(super) lock_held: bool,
    /// Nothing has completed a pass for the whole stall window.
    pub(super) stalled: bool,
    /// How long ago the last pass was turned away, when one was.
    pub(super) prevented_age_seconds: Option<i64>,
}

impl JanitorHealth {
    /// `state_observed` is false when the host would not answer with its
    /// janitor state at all; nothing here may be judged from silence.
    pub(super) fn read(
        reading: &host_disk::DiskReading,
        now: DateTime<Utc>,
        state_observed: bool,
        stall_after_seconds: Option<i64>,
        cleanup_success_age_seconds: Option<i64>,
    ) -> Self {
        // How long the janitor has been PREVENTED rather than silent. A workload
        // holds the run lock in shared mode for its whole duration, by design, and
        // every pass that starts meanwhile answers `lock_busy` — the modelled
        // answer, not a fault.
        let cleanup_prevented_age_seconds = reading
            .state
            .last_prevented_at
            .as_deref()
            .and_then(|stamp| DateTime::parse_from_rfc3339(&stamp.replace('Z', "+00:00")).ok())
            .map(|stamp| (now - stamp.with_timezone(&Utc)).num_seconds());
        // A pass prevented within the same window the stall is measured over is a
        // janitor that is still running and still being turned away, so the age of
        // its last success says nothing about its health. Only silence does.
        //
        // Without this, a host that runs one long job while the in-process
        // janitor polls every ten seconds would let the success age pass its
        // limit — because a prevented pass records nothing — and switch
        // `claiming` off on a host with free space above its watermark and
        // `disk_pressure_unresolved: false`, refusing new work because it is
        // doing work.
        let cleanup_prevented = match (stall_after_seconds, cleanup_prevented_age_seconds) {
            (Some(limit), Some(age)) => age <= limit,
            _ => false,
        };
        // Being turned away is healthy for as long as somebody is taking turns.
        // Being turned away while nothing has got through for the whole window the
        // stall is measured over is not being turned away — it is a lock that is
        // held, and it has a different remedy from every other condition here:
        // find the holder (`space report`'s `cleanup_lock.holders` names the pid) and
        // deal with THAT process. See [`DISK_CLEANUP_LOCK_HELD`].
        let disk_cleanup_lock_held = state_observed
            && cleanup_prevented
            && match (stall_after_seconds, cleanup_success_age_seconds) {
                (None, _) => false,
                (Some(_), None) => true,
                (Some(limit), Some(age)) => age > limit,
            };
        let disk_cleanup_stalled = state_observed
            && !cleanup_prevented
            && match (stall_after_seconds, cleanup_success_age_seconds) {
                (None, _) => false,
                // Declared, armed, and no completed pass on record at all. Reported
                // rather than excused: a janitor that has never finished a pass is
                // the fifteen-day case exactly, and the state file being absent or
                // fresh says nothing about whether the thing ever worked.
                (Some(_), None) => true,
                (Some(limit), Some(age)) => age > limit,
            };
        Self {
            prevented_age_seconds: cleanup_prevented_age_seconds,
            lock_held: disk_cleanup_lock_held,
            stalled: disk_cleanup_stalled,
        }
    }
}
