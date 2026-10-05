//! How old the knowledge is: the newest look at one fact, and its age.
//!
//! No window decides when a look stops counting. A look describes the moment
//! it was taken; a reader is told that moment and how long ago it was, and a
//! judgement that needs the present asks again or compares the look's
//! content against what has changed since.

use std::time::Duration;

use chrono::{DateTime, Utc};

use super::observation::Observation;
use super::store::load;

/// What the fleet knows about one fact.
#[derive(Debug, Clone)]
pub enum Freshness {
    /// The newest observation of the fact, of whatever age.
    Seen(Observation),
    /// No observation exists for this fact. Neither a failure nor a pass.
    Never,
}

/// The newest look at `fact` across all vantages.
///
/// The newest observation answers, because the question this serves -- "what
/// did anyone last see" -- is answered by the most recent look from anywhere;
/// a caller that needs one specific vantage is asking a different question
/// and should read [`load`] and filter it. A row whose stamp will not parse
/// answers only when no dated row exists: somebody looked, so `Never` would
/// be false.
pub fn freshness(fact: &str) -> Freshness {
    freshness_in(&load(), fact)
}

/// [`freshness`] against records already in hand.
///
/// Reuses one loaded set for a table instead of re-reading the store per cell.
pub fn freshness_in(records: &[Observation], fact: &str) -> Freshness {
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
    match (newest, undated) {
        (Some((_, row)), _) | (None, Some(row)) => Freshness::Seen(row.clone()),
        (None, None) => Freshness::Never,
    }
}

/// How long ago the look happened, clamped at zero.
///
/// A stamp in the future is clock skew between two hosts, not a prophecy. It
/// reads as `just now` rather than as an enormous negative age, because the
/// alternative is a column that renders a skewed laptop as the freshest thing
/// in the fleet or as gibberish, and neither tells an operator about the skew.
pub(crate) fn age(row: &Observation) -> Option<Duration> {
    let moment = row.moment()?;
    Utc::now()
        .signed_duration_since(moment)
        .to_std()
        .ok()
        .or(Some(Duration::ZERO))
}
