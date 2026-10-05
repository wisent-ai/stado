//! One column's worth of knowledge: how long ago the newest look was taken,
//! in the shortest unit that says something, or that nobody ever looked.

use std::time::Duration;

use super::observation::Observation;
use super::staleness::{age, freshness, freshness_in, Freshness};

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

/// One column's worth: `just now`, `14m ago`, `12d ago`, `undated`, `never`.
///
/// The age is shown as an age of the look — `ago` — because it is a fact
/// about the fleet's knowledge of the service, not about the service.
/// `never` is spelled out because an empty cell reads as "fine" to every
/// operator alive.
pub fn render(freshness: &Freshness) -> String {
    match freshness {
        Freshness::Seen(row) => match age(row) {
            Some(span) if span.as_secs() < MINUTE => "just now".to_string(),
            Some(span) => format!("{} ago", compact(span)),
            None => "undated".to_string(),
        },
        Freshness::Never => "never".to_string(),
    }
}

/// [`render`] over [`freshness`].
pub fn describe(fact: &str) -> String {
    render(&freshness(fact))
}

/// [`describe`] against records already in hand, for a table that loads the
/// file once and then asks about every row.
pub fn describe_in(records: &[Observation], fact: &str) -> String {
    render(&freshness_in(records, fact))
}
