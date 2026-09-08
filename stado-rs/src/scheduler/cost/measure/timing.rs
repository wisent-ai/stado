//! Wall-clock measurement: ISO-8601 timestamp parsing and the
//! started -> finished span every cost row is priced against.

use chrono::{DateTime, Utc};

use crate::models::Job;

/// Parse an ISO-8601 timestamp with a trailing "Z" or offset. Python
/// `_parse_iso` (`fromisoformat(ts.replace("Z", "+00:00"))`).
fn parse_iso(ts: Option<&str>) -> Option<DateTime<Utc>> {
    let ts = ts?;
    if ts.is_empty() {
        return None;
    }
    DateTime::parse_from_rfc3339(ts)
        .ok()
        .map(|dt| dt.with_timezone(&Utc))
}

/// started -> (completed or failed). None if either side missing.
/// Python `_wall_seconds`.
pub(in crate::scheduler::cost) fn wall_seconds(job: &Job) -> Option<f64> {
    let start = parse_iso(job.started_at.as_deref())?;
    let end =
        parse_iso(job.completed_at.as_deref()).or_else(|| parse_iso(job.failed_at.as_deref()))?;
    Some(((end - start).num_milliseconds() as f64 / 1000.0).max(0.0))
}
