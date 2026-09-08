//! The joins and transitions: pure functions over already-loaded documents,
//! so the truth table is exercisable without a store, a network, or a sick
//! host.

use chrono::{DateTime, Utc};

use super::records::{RefusalRecord, RefusalSummary, SilenceRecord};
use super::{DEFAULT_SILENCE_THRESHOLD_SECONDS, SILENCE_THRESHOLD_ENV};

/// Beacon age past which a host counts as silent.
///
/// The single reader of `STADO_SILENCE_THRESHOLD_SECONDS` in the crate. A
/// second parse elsewhere is how two commands come to disagree about
/// whether a host is down. Values that are not a positive integer resolve
/// to [`DEFAULT_SILENCE_THRESHOLD_SECONDS`] rather than disabling the
/// detector: a typo in a launchd plist must not silently switch off the
/// thing that notices outages.
pub fn silence_threshold_seconds() -> i64 {
    std::env::var(SILENCE_THRESHOLD_ENV)
        .ok()
        .and_then(|raw| raw.trim().parse::<i64>().ok())
        .filter(|seconds| *seconds > 0)
        .unwrap_or(DEFAULT_SILENCE_THRESHOLD_SECONDS)
}

/// Whether a host with this newest beacon counts as silent at `now`.
///
/// No beacon at all is silent: a host that has never published is not a
/// host that is fine. A beacon stamped in the future is NOT silent — clock
/// skew on the publisher is not an outage, and reporting it as one sends an
/// operator to the wrong machine.
pub fn beacon_is_silent(
    newest_beacon_at: Option<DateTime<Utc>>,
    now: DateTime<Utc>,
    threshold_seconds: i64,
) -> bool {
    match newest_beacon_at {
        None => true,
        Some(at) => (now - at).num_seconds() > threshold_seconds,
    }
}

/// Open a silence that began at `started_at`.
pub fn open_record(
    host: &str,
    started_at: DateTime<Utc>,
    observer: &str,
    first_reader_error: Option<&str>,
) -> SilenceRecord {
    SilenceRecord {
        host: host.to_string(),
        started_at,
        ended_at: None,
        duration_seconds: None,
        first_reader_error: first_reader_error.map(str::to_string),
        observed_by: vec![observer.to_string()],
    }
}

/// Fold one more observation into an open record; `true` when it changed.
///
/// `first_reader_error` is written once and never overwritten — the point
/// of the field is which subsystem noticed FIRST, and a later reader
/// clobbering it turns the record into a report of whoever ran most
/// recently.
pub fn merge_observation(
    record: &mut SilenceRecord,
    observer: &str,
    first_reader_error: Option<&str>,
) -> bool {
    let mut changed = false;
    if !record.observed_by.iter().any(|seen| seen == observer) {
        record.observed_by.push(observer.to_string());
        changed = true;
    }
    if record.first_reader_error.is_none() {
        if let Some(error) = first_reader_error {
            record.first_reader_error = Some(error.to_string());
            changed = true;
        }
    }
    changed
}

/// Close an open record at `ended_at`; `false` when it was already closed.
///
/// A close stamped before the open is clamped to a zero duration rather
/// than reported as negative time: the fresher beacon proves the host is
/// back, and the only thing a negative number would document is the skew
/// between two clocks.
pub fn close_record(record: &mut SilenceRecord, ended_at: DateTime<Utc>) -> bool {
    if record.ended_at.is_some() {
        return false;
    }
    record.ended_at = Some(ended_at);
    record.duration_seconds = Some((ended_at - record.started_at).num_seconds().max(0));
    true
}

/// Count refusals inside `window_seconds` back from `now`, per reason.
///
/// Records stamped in the future are counted: they are refusals that
/// happened, and dropping them because a publisher's clock runs fast would
/// hide exactly the fleet-wide condition this is for.
pub fn summarize_refusals(
    records: &[RefusalRecord],
    now: DateTime<Utc>,
    window_seconds: i64,
) -> RefusalSummary {
    let mut summary = RefusalSummary::empty(window_seconds);
    for record in records {
        if (now - record.at).num_seconds() > window_seconds {
            continue;
        }
        summary.count += 1;
        *summary.reasons.entry(record.reason.clone()).or_insert(0) += 1;
    }
    summary
}
