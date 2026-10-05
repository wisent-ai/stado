//! The promise a worker writes into each running job it holds: the time by
//! which it will renew that job's lease again.
//!
//! The worker renews every job it runs once per poll period (the period the
//! operator declared with `--poll-seconds`). A renewal can land later than
//! one period after the previous one — the runtime was busy, the store was
//! slow — and the worker measures by how much, every time. Its promise is the
//! poll period, plus the longest such lateness it has measured in this
//! process, plus how long its last renewal write took. Nothing here is
//! chosen: the period is the operator's, the rest is measured.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

/// The longest a renewal has landed after its period ended, in this process.
static LONGEST_LATENESS_MS: AtomicU64 = AtomicU64::new(0);
/// How long the last renewal write took.
static LAST_WRITE_MS: AtomicU64 = AtomicU64::new(0);

fn millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

/// One renewal happened: `since_previous` after the previous renewal of the
/// same job began (None for a job's first), and its write took `write`.
pub fn record_renewal(poll: Duration, since_previous: Option<Duration>, write: Duration) {
    if let Some(since) = since_previous {
        LONGEST_LATENESS_MS.fetch_max(millis(since.saturating_sub(poll)), Ordering::Relaxed);
    }
    LAST_WRITE_MS.store(millis(write), Ordering::Relaxed);
}

/// How far ahead this worker's next renewal is promised, or None when this
/// process has declared no poll period (it is not a worker, and a job it
/// claimed would carry no promise anyone could hold it to).
pub fn lease_promise() -> Option<chrono::Duration> {
    let poll = crate::providers::local::agent::POLL.get().copied()?;
    let measured = Duration::from_millis(
        LONGEST_LATENESS_MS
            .load(Ordering::Relaxed)
            .saturating_add(LAST_WRITE_MS.load(Ordering::Relaxed)),
    );
    chrono::Duration::from_std(poll + measured).ok()
}
