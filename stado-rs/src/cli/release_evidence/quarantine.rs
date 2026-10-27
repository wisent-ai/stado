//! The host's quarantine map as evidence: what each record failed for, which
//! digest the registry still desires, and what the whole table adds up to.

use serde_json::{json, Value};

use crate::release_agent::HostReleaseState;
use crate::release_cause::{self, QuarantineCause};

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
    // A host with no release state holds no quarantine record to show.
    state
        .into_iter()
        .flat_map(|state| state.quarantined.iter())
        .map(|(digest, record)| {
            let classified = record.classification();
            json!({
                "digest": digest,
                "reason": record.reason,
                "quarantined_at": record.quarantined_at.to_rfc3339(),
                "is_desired_digest": desired_digest == Some(digest.as_str()),
                "cause": classified.cause.as_str(),
                "evidence": classified.evidence,
                "remedy": classified.cause.remedy(),
                // Whether this row needs an operator at all. A cause that
                // says nothing about the candidate is retired by the agent
                // itself on a later tick, and an operator reading a table
                // of refusals has to be able to see which of them are
                // already recovering.
                "agent_retires": !classified.cause.holds_the_candidate(),
            })
        })
        .collect()
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
    // A row without a cause is one nobody classified; a cause word that does
    // not read back is this crate's own serialization disagreeing with
    // itself, and is named rather than counted as unclassified.
    let mut unreadable: Vec<String> = Vec::new();
    let causes: Vec<QuarantineCause> = quarantined
        .iter()
        .filter_map(|entry| match entry.get("cause") {
            None | Some(Value::Null) => Some(QuarantineCause::Unclassified),
            Some(Value::String(word)) => match parse_cause(word) {
                Ok(cause) => Some(cause),
                Err(refusal) => {
                    unreadable.push(format!("{}: {refusal}", entry["digest"]));
                    None
                }
            },
            Some(other) => {
                unreadable.push(format!("{}: quarantine cause is not text: {other}", entry["digest"]));
                None
            }
        })
        .collect();
    let total = quarantined.len();
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
            .filter(|(cause, _)| !cause.is_classified())
            .map(|(_, count)| *count)
            .sum::<usize>(),
        "unreadable_causes": unreadable,
    })
}

/// The cause behind a word this module just wrote.
///
/// The summary counts the rendered entries rather than the records, so that the
/// table and the totals above it can never disagree about one row. That means
/// reading the word back, and the word is this crate's own serialization; one
/// that does not read back is refused naming it.
fn parse_cause(word: &str) -> Result<QuarantineCause, String> {
    serde_json::from_value(Value::String(word.to_string()))
        .map_err(|error| format!("quarantine cause {word:?} does not read back: {error}"))
}
