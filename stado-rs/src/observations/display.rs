//! One column's worth of freshness: an age turned into the shortest unit that
//! says something, and the words a reader is given when it is out of date or
//! was never taken at all.

use std::time::Duration;

use super::observation::Observation;
use super::staleness::{age, freshness, freshness_in, Freshness, DEFAULT_TTL};

const MINUTE: u64 = 60;
const HOUR: u64 = 60 * MINUTE;
const DAY: u64 = 24 * HOUR;

/// A duration in the largest unit that still says something: `45s`, `14m`,
/// `3h`, `12d`. Twelve days is the number this fleet has to be able to read at
/// a glance.
fn compact(span: Duration) -> String {
    let seconds = span.as_secs();
    if seconds < MINUTE {
        format!("{seconds}s")
    } else if seconds < HOUR {
        format!("{}m", seconds / MINUTE)
    } else if seconds < DAY {
        format!("{}h", seconds / HOUR)
    } else {
        format!("{}d", seconds / DAY)
    }
}

/// One column's worth of freshness: `just now`, `14m ago`, `stale (3h)`,
/// `never`.
///
/// `stale` is spelled out as a word rather than shown as a bare age, because
/// an age alone is read as a fact about the service and this is a fact about
/// the fleet's knowledge of it. `never` is the same shape for the same reason:
/// an empty cell reads as "fine" to every operator alive.
pub fn render(freshness: &Freshness) -> String {
    match freshness {
        Freshness::Fresh(row) => match age(row) {
            Some(span) if span.as_secs() < MINUTE => "just now".to_string(),
            Some(span) => format!("{} ago", compact(span)),
            None => "just now".to_string(),
        },
        Freshness::Stale(row) => match age(row) {
            Some(span) => format!("stale ({})", compact(span)),
            None => "stale (undated)".to_string(),
        },
        Freshness::Never => "never".to_string(),
    }
}

/// [`render`] over [`freshness`] at [`DEFAULT_TTL`], for the display paths
/// that all want the same question asked the same way.
pub fn describe(fact: &str) -> String {
    render(&freshness(fact, DEFAULT_TTL))
}

/// [`describe`] against records already in hand, for a table that loads the
/// file once and then asks about every row.
pub fn describe_in(records: &[Observation], fact: &str) -> String {
    render(&freshness_in(records, fact, DEFAULT_TTL))
}
