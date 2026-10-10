//! The promise a worker writes into each running job it holds: the time by
//! which it will renew that job's lease again.
//!
//! The worker renews every job it runs once per poll period (the period the
//! operator declared with `--poll-seconds`). A renewal can land later than
//! one period after the previous one — the runtime was busy, the store was
//! slow — and the worker measures by how much, every time. Its promise is the
//! poll period, plus the lateness of its newest renewal, plus how long that
//! renewal's write took. Nothing here is chosen: the period is the
//! operator's, the rest is measured.
//!
//! The newest lateness, not the longest this process ever measured: one
//! renewal held for hours by a store that did not answer made every later
//! lease of the process promise those hours, so when that worker died its
//! jobs — and every schedule whose skip policy waits on them — stayed held
//! for as long, with no process behind them. A promise that follows the
//! worker's current pace can lapse under a live process after one unusually
//! late renewal; the slot then finds its job back in the queue and takes it
//! back on its next tick (`slots::lifecycle::tick`), so no second process
//! is ever started for it.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

/// How late the newest renewal landed after its period ended, in this process.
static LAST_LATENESS_MS: AtomicU64 = AtomicU64::new(0);
/// How long the last renewal write took.
static LAST_WRITE_MS: AtomicU64 = AtomicU64::new(0);

fn millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

/// One renewal happened: `since_previous` after the previous renewal of the
/// same job began (None for a job's first), and its write took `write`.
pub fn record_renewal(poll: Duration, since_previous: Option<Duration>, write: Duration) {
    if let Some(since) = since_previous {
        LAST_LATENESS_MS.store(millis(since.saturating_sub(poll)), Ordering::Relaxed);
    }
    LAST_WRITE_MS.store(millis(write), Ordering::Relaxed);
}

/// How far ahead this worker's next renewal is promised, or None when this
/// process has declared no poll period (it is not a worker, and a job it
/// claimed would carry no promise anyone could hold it to).
pub fn lease_promise() -> Option<chrono::Duration> {
    let poll = crate::providers::local::agent::POLL.get().copied()?;
    let measured = Duration::from_millis(
        LAST_LATENESS_MS
            .load(Ordering::Relaxed)
            .saturating_add(LAST_WRITE_MS.load(Ordering::Relaxed)),
    );
    chrono::Duration::from_std(poll + measured).ok()
}
