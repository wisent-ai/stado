//! The pacing control a tick applies before it spends anything: the
//! per-job dispatch-backoff window (how long a failed job is skipped).

use chrono::{DateTime, Duration, Utc};

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
