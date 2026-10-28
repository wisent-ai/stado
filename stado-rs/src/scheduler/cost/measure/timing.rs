//! Wall-clock measurement: ISO-8601 timestamp parsing and the
//! started -> finished span every cost row is priced against.

use chrono::{DateTime, Utc};

use crate::models::Job;

/// One job timestamp with a trailing "Z" or offset. Absent or empty is
/// `None`; a stamp that does not parse is refused naming the field, because
/// read as absent the job's run vanished from every cost total.
fn parse_iso(field: &str, ts: Option<&str>) -> Result<Option<DateTime<Utc>>, String> {
    let Some(ts) = ts.filter(|ts| !ts.is_empty()) else {
        return Ok(None);
    };
    DateTime::parse_from_rfc3339(ts)
        .map(|dt| Some(dt.with_timezone(&Utc)))
        .map_err(|error| format!("{field} {ts:?} is not RFC 3339: {error}"))
}

/// started -> (completed or failed). `None` when the job has no start or no
/// finish; a stamp that does not parse, or a finish before the start, is
/// refused with the reason instead of being priced as nothing or as zero.
pub(in crate::scheduler::cost) fn wall_seconds(job: &Job) -> Result<Option<f64>, String> {
    let Some(start) = parse_iso("started_at", job.started_at.as_deref())? else {
        return Ok(None);
    };
    let end = match parse_iso("completed_at", job.completed_at.as_deref())? {
        Some(end) => end,
        None => match parse_iso("failed_at", job.failed_at.as_deref())? {
            Some(end) => end,
            None => return Ok(None),
        },
    };
    if end < start {
        return Err(format!(
            "it finished at {end}, before it started at {start}"
        ));
    }
    Ok(Some((end - start).as_seconds_f64()))
}
