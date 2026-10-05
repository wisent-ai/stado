//! Whether the host's disk janitor is healthy, held, or silent.
//!
//! A periodic janitor writer states when its next pass will have written the
//! state file, and names its process. The janitor is running while one of
//! those promises holds or the process that made one is alive: a promise
//! measured from past passes can be overrun by a slow one, and a process
//! being replaced is briefly gone while its promise still holds. A pass
//! turned away while the janitor runs reports a held lock; a janitor nobody
//! runs, one that never finished a pass, or a last pass that ran and did not
//! succeed reports a stalled janitor. These states require different remedies.

use chrono::{DateTime, Utc};

use crate::deploy::host_disk;

/// What the two janitor gates read.
pub(super) struct JanitorHealth {
    /// A workload holds the run lock and the last pass was turned away.
    pub(super) lock_held: bool,
    /// Nobody keeps a promise to make the next pass, no pass has ever
    /// succeeded, or the last pass ran and did not succeed.
    pub(super) stalled: bool,
    /// How long ago the last pass was turned away, when one was.
    pub(super) prevented_age_seconds: Option<i64>,
}

fn parsed(stamp: Option<&str>) -> Option<DateTime<Utc>> {
    stamp
        .and_then(|stamp| DateTime::parse_from_rfc3339(&stamp.replace('Z', "+00:00")).ok())
        .map(|stamp| stamp.with_timezone(&Utc))
}

impl JanitorHealth {
    /// `state_observed` is false when the host would not answer with its
    /// janitor state at all; nothing here may be judged from silence.
    pub(super) fn read(
        reading: &host_disk::DiskReading,
        now: DateTime<Utc>,
        state_observed: bool,
    ) -> Self {
        let state = &reading.state;
        let prevented_age_seconds =
            parsed(state.last_prevented_at.as_deref()).map(|stamp| (now - stamp).num_seconds());
        // Every periodic writer states when its next pass will have written
        // the state file. A janitor older than that statement states nothing.
        let promise_kept = state
            .promises
            .iter()
            .any(|promise| parsed(promise.next_pass_by.as_deref()).is_some_and(|by| now <= by));
        let promiser_alive = reading.live_stado_pids.as_ref().is_some_and(|live| {
            state
                .promises
                .iter()
                .filter_map(|promise| promise.pid)
                .any(|pid| live.contains(&pid))
        });
        let running = promise_kept || promiser_alive;
        let last_success = parsed(state.last_success_at.as_deref());
        // A pass that ran — was not turned away — and finished without
        // stamping a success at or after its own start.
        let last_pass_failed = !state.prevented
            && match (parsed(state.last_pass_at.as_deref()), last_success) {
                (Some(pass), Some(success)) => success < pass,
                _ => false,
            };
        // Being turned away is the modelled answer while a workload holds the
        // shared lock for its whole duration; it has its own remedy: find the
        // holder (`space report`'s `cleanup_lock.holders` names the pid) and
        // deal with THAT process. See [`DISK_CLEANUP_LOCK_HELD`].
        let lock_held = state_observed && state.prevented && running;
        // A janitor that has never finished a pass is reported rather than
        // excused: the state file being present says nothing about whether
        // the thing ever worked.
        let stalled = state_observed
            && (!running || last_success.is_none() && !state.prevented || last_pass_failed);
        Self {
            prevented_age_seconds,
            lock_held,
            stalled,
        }
    }
}
