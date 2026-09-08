//! The two pacing controls a tick applies before it spends anything: the
//! per-job dispatch-backoff window (how long a failed job is skipped) and
//! the per-tick dispatch cap (how many launches a tick may attempt).

use chrono::{DateTime, Duration, Utc};

use crate::config;
use crate::models::Job;

/// Backoff schedule by attempt count; index = attempt count.
/// Each entry is the minimum minutes since last_dispatch_attempt before we
/// retry.
pub const DISPATCH_BACKOFF_MINUTES: [i64; 7] = [0, 1, 5, 15, 30, 60, 120];
pub const MAX_DISPATCH_BACKOFF_MINUTES: i64 = 240;

/// True if this job is past its dispatch-backoff window.
/// Python `_backoff_due`.
pub fn backoff_due(job: &Job, now_utc: DateTime<Utc>) -> bool {
    let attempts = job.dispatch_attempts;
    if attempts <= 0 {
        return true;
    }
    let idx = (attempts as usize).min(DISPATCH_BACKOFF_MINUTES.len() - 1);
    let wait_minutes = DISPATCH_BACKOFF_MINUTES[idx].min(MAX_DISPATCH_BACKOFF_MINUTES);
    let Some(last) = &job.last_dispatch_attempt else {
        return true;
    };
    if last.is_empty() {
        return true;
    }
    // Python `datetime.fromisoformat(last.replace("Z", "+00:00"))`.
    let Ok(last_dt) = DateTime::parse_from_rfc3339(&last.replace('Z', "+00:00")) else {
        return true;
    };
    now_utc - last_dt.with_timezone(&Utc) >= Duration::minutes(wait_minutes)
}

/// Autoscale dispatch cap with queue depth. Python `_dynamic_per_tick_cap`.
///
/// Defaults to MAX_SCHEDULE_PER_TICK (4) for shallow queues, scales up for
/// larger bursts so a 723-job batch doesn't drip-feed at 4-per-tick. Upper
/// bound aligned with the multi-region preemptible quota envelope (5
/// regions x ~36 spot GPUs = ~180 ceiling).
pub fn dynamic_per_tick_cap(queue_depth: i64) -> i64 {
    let base = config::MAX_SCHEDULE_PER_TICK;
    if queue_depth <= base * 2 {
        return base;
    }
    // cap=25 fits 60s tick budget
    (base + (queue_depth - base * 2) / 4 + 4).min(25)
}
