//! How old the knowledge is: the window an observation speaks for, the three
//! answers a reader may get, and the lookup that picks between them.

use std::time::Duration;

use chrono::{DateTime, Utc};

use super::observation::Observation;
use super::store::load;

/// How long an observation speaks for the present.
///
/// One hour. A closed lid, a killed process and a revoked forward all happen
/// in under a second, so no TTL makes a stored observation equal to a live
/// probe; this is the window the fleet accepts being wrong in, chosen so that
/// a routine sweep keeps every fact green and a fleet nobody is sweeping goes
/// visibly amber within the working hour rather than silently in twelve days.
pub const DEFAULT_TTL: Duration = Duration::from_secs(3600);

/// How old the fleet's knowledge of one fact is.
///
/// `Stale` carries the observation rather than discarding it because the last
/// thing anyone saw is the most useful thing an operator can be told after
/// "this is out of date" -- but it is handed back in a variant that cannot be
/// mistaken for the present by a caller pattern-matching on it.
#[derive(Debug, Clone)]
pub enum Freshness {
    /// Observed inside the TTL. Safe to act on.
    Fresh(Observation),
    /// The most recent observation, older than the TTL. History, not state.
    Stale(Observation),
    /// No machine has ever recorded a look at this fact. Not a failure, and
    /// not a pass; the absence of evidence, which is what the twelve days were.
    Never,
}

/// What the fleet knows about one fact right now, and how old that knowledge
/// is.
///
/// The newest observation across all vantages answers, because the question
/// this serves -- "may I rely on this" -- is answered by the most recent look
/// from anywhere; a caller that needs one specific vantage is asking a
/// different question and should read [`load`] and filter it.
///
/// A row whose stamp will not parse is reported `Stale`, never `Fresh` and
/// never `Never`. Somebody looked, so `Never` would be false; the age is
/// unknown, and unknown age is not freshness.
pub fn freshness(fact: &str, ttl: Duration) -> Freshness {
    freshness_in(&load(), fact, ttl)
}

/// [`freshness`] against records already in hand.
///
/// A table asks this once per row. Re-reading and re-parsing the whole file
/// for each cell would make the cost of showing freshness scale with the size
/// of the fleet, and a column that gets slower the more services you run is a
/// column somebody eventually deletes -- which is how the fact lost its reader
/// the first time.
pub fn freshness_in(records: &[Observation], fact: &str, ttl: Duration) -> Freshness {
    let mut newest: Option<(DateTime<Utc>, &Observation)> = None;
    let mut undated: Option<&Observation> = None;
    for row in records {
        if row.fact != fact {
            continue;
        }
        match row.moment() {
            None => undated = Some(row),
            Some(moment) => {
                if newest.as_ref().is_none_or(|(held, _)| moment >= *held) {
                    newest = Some((moment, row));
                }
            }
        }
    }
    let Some((_, row)) = newest else {
        return match undated {
            Some(row) => Freshness::Stale(row.clone()),
            None => Freshness::Never,
        };
    };
    match age(row) {
        Some(span) if span <= ttl => Freshness::Fresh(row.clone()),
        _ => Freshness::Stale(row.clone()),
    }
}

/// How long ago the look happened, clamped at zero.
///
/// A stamp in the future is clock skew between two hosts, not a prophecy. It
/// reads as `just now` rather than as an enormous negative age, because the
/// alternative is a column that renders a skewed laptop as the freshest thing
/// in the fleet or as gibberish, and neither tells an operator about the skew.
pub(super) fn age(row: &Observation) -> Option<Duration> {
    let moment = row.moment()?;
    Utc::now()
        .signed_duration_since(moment)
        .to_std()
        .ok()
        .or(Some(Duration::ZERO))
}
