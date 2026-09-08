//! The host's quarantine map as evidence: what each record failed for, which
//! digest the registry still desires, and what the whole table adds up to.

use serde_json::{json, Value};

use crate::release_agent::{HostReleaseState, QuarantineRecord};
use crate::release_cause::{self, Classification, QuarantineCause};

/// The named cause one quarantine record carries, deriving it from the reason
/// when the record has none of its own.
///
/// Every record already on the fleet was written before the agent classified
/// anything, so reading only the stored field would report this host's entire
/// month of history as unclassified. Re-deriving costs one pass over a string
/// the command already holds and is idempotent: the stored name came from the
/// same classifier over a superset of the same text, so a record that really
/// is unclassified stays unclassified.
///
/// What re-derivation cannot recover is what the truncated reason no longer
/// contains. That is a property of the old records, not of this function, and
/// it is why the agent now classifies from the whole log at the moment it
/// quarantines.
pub(crate) fn record_cause(record: &QuarantineRecord) -> Classification {
    if record.cause.is_classified() {
        return Classification {
            cause: record.cause,
            evidence: record.evidence.clone(),
        };
    }
    release_cause::classify(&record.reason)
}

/// The host's quarantine map, each entry told what it failed for and whether it
/// is the digest the registry currently desires.
///
/// `is_desired_digest` is the whole point of showing the map: a quarantined
/// digest that nobody desires is history, and the one that matches desired
/// state is a rollout that will be skipped on every pass until someone
/// clears it.
pub(super) fn quarantine_entries(
    state: Option<&HostReleaseState>,
    desired_digest: Option<&str>,
) -> Vec<Value> {
    state.map_or_else(Vec::new, |state| {
        state
            .quarantined
            .iter()
            .map(|(digest, record)| {
                let classified = record_cause(record);
                json!({
                    "digest": digest,
                    "reason": record.reason,
                    "quarantined_at": record.quarantined_at.to_rfc3339(),
                    "is_desired_digest": desired_digest == Some(digest.as_str()),
                    "cause": classified.cause.as_str(),
                    "evidence": classified.evidence,
                    "remedy": classified.cause.remedy(),
                })
            })
            .collect()
    })
}

/// How many quarantines share each cause, which one dominates, and what to do
/// about it.
///
/// The table this sits above had twenty rows of truncated stderr on the host
/// that prompted it, and reading it was the operator's job: several of those
/// rows were one thing, and nothing said so. This is that sentence, computed.
///
/// `unclassified` is carried in `causes` like any other count and excluded from
/// `dominant`. On a host with history it is usually the largest bucket, and an
/// operator asking what dominates wants a cause they can act on -- but hiding
/// the count would misrepresent how much of the table is actually understood.
pub(super) fn cause_summary(quarantined: &[Value]) -> Value {
    let causes: Vec<QuarantineCause> = quarantined
        .iter()
        .map(|entry| {
            entry["cause"]
                .as_str()
                .map_or(QuarantineCause::Unclassified, parse_cause)
        })
        .collect();
    let total = causes.len();
    let tally = release_cause::tally(causes);
    let dominant = release_cause::dominant(&tally);
    json!({
        "total": total,
        "causes": tally
            .iter()
            .map(|(cause, count)| json!({
                "cause": cause.as_str(),
                "count": count,
                "remedy": cause.remedy(),
            }))
            .collect::<Vec<Value>>(),
        "dominant_cause": dominant.map(|(cause, _)| cause.as_str()),
        "dominant_count": dominant.map(|(_, count)| count),
        "unclassified": tally
            .iter()
            .find(|(cause, _)| !cause.is_classified())
            .map_or(0, |(_, count)| *count),
    })
}

/// The cause behind a word this module just wrote.
///
/// The summary counts the rendered entries rather than the records, so that the
/// table and the totals above it can never disagree about one row. That means
/// reading the word back, and the word is this crate's own serialization.
fn parse_cause(word: &str) -> QuarantineCause {
    serde_json::from_value(Value::String(word.to_string())).unwrap_or(QuarantineCause::Unclassified)
}
