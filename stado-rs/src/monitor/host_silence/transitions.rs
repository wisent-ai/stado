//! The joins and transitions: pure functions over already-loaded documents,
//! so the truth table is exercisable without a store, a network, or a sick
//! host.

use chrono::{DateTime, Utc};

use super::records::{RefusalRecord, RefusalSummary, SilenceRecord};

/// Whether a host whose newest beacon promised its next one by `next_by`
/// counts as silent at `now`.
///
/// A host is silent once the time its own publisher promised has passed
/// (`next_by`: the publisher's period plus its last collection, or the
/// `stale_after_seconds` an older beacon stated). No promise at all — no
/// beacon, or one that states none — is silent: nothing says the host is
/// still speaking. No window of a reader's choosing decides it, so every
/// command that asks agrees.
pub fn beacon_is_silent(next_by: Option<DateTime<Utc>>, now: DateTime<Utc>) -> bool {
    next_by.is_none_or(|by| now > by)
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

/// Count refusals at or after `since` (all of them when `None`), per reason.
///
/// Records stamped in the future are counted: they are refusals that
/// happened, and dropping them because a publisher's clock runs fast would
/// hide exactly the fleet-wide condition this is for.
pub fn summarize_refusals(
    records: &[RefusalRecord],
    since: Option<DateTime<Utc>>,
) -> RefusalSummary {
    let mut summary = RefusalSummary::since(since);
    for record in records {
        if since.is_some_and(|since| record.at < since) {
            continue;
        }
        summary.count += 1;
        *summary.reasons.entry(record.reason.clone()).or_insert(0) += 1;
    }
    summary
}
