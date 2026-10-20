//! The services of other hosts this host's clients stand on, and what this
//! host did about them.
//!
//! The host that runs a service judges it by the question its catalog asks
//! and recovers it when it stands. A host that runs an older catalog never
//! asks the question that would find it standing, and a host whose own
//! Stado stands on the service it would judge never reaches the judgement:
//! a vault wedged that way held every other host's requests for hours while
//! each of their resolvers recorded the waits and refused new requests, and
//! nothing acted. Here every host reads its own resolver's held waits on
//! each beacon pass ([`crate::cli::resolver::stood_elsewhere`]); a service
//! of another host that holds them past the declared refresh interval is
//! recovered from this host through the same declared repair steps and
//! restart its own host runs, both carried by the registry SSH channel and
//! neither needing that host's Stado, and the record goes to this host's
//! recovery register naming both hosts.
//!
//! A standing with completed declared repairs and restart is acted on once:
//! the register keeps `stood_since`, the acceptance of the oldest held
//! request, and a later pass reports that recovery. A refused repair or
//! restart is retried. When the register cannot append a completed recovery,
//! this process remembers the outcome for that same standing until it ends.
//! A restart ends the held connections, so a subsequent standing has a new
//! oldest request.

use serde_json::{json, Value};

use crate::cli::resolver::Stood;

use super::recover::{recover, Peer};
use super::{episode, register, Reading};

/// Every service of another host that stands for this host's clients, each
/// with what this pass did: the recovery it ran, the earlier recovery of
/// the same standing, or why it could not act. A waiting list this host
/// cannot read is said instead of the list.
pub async fn recoveries(home: &std::path::Path, this_host: &str) -> Value {
    let stood = match crate::cli::resolver::stood_elsewhere(this_host) {
        Ok(stood) => stood,
        Err(unreadable) => return json!({ "unreadable": unreadable }),
    };
    episode::retain_peers(&stood);
    let mut blocks = Vec::with_capacity(stood.len());
    for standing in &stood {
        let mut block = json!({
            "service": standing.service,
            "host": standing.host,
            "since": standing.since,
            "waited_seconds": standing.waited_seconds,
            "bound_seconds": standing.bound_seconds,
            "held": standing.held,
            "oldest": standing.sentence,
        });
        let (field, outcome) = act(home, this_host, standing).await;
        block[field] = outcome;
        blocks.push(block);
    }
    Value::Array(blocks)
}

/// What this host does about one standing: `recovery` with the record,
/// `already_recovered` with the earlier record of the same standing, or
/// `not_recovered` with the reason it could not act.
async fn act(home: &std::path::Path, this_host: &str, standing: &Stood) -> (&'static str, Value) {
    let entry = match crate::deploy::service_catalog::lookup(&standing.service) {
        Ok(Some(entry)) => entry,
        Ok(None) => {
            return (
                "not_recovered",
                json!(format!(
                    "{} is not a service the catalog declares",
                    standing.service
                )),
            )
        }
        Err(unreadable) => return ("not_recovered", json!(unreadable)),
    };
    let Some(declared) = entry.answering.as_ref() else {
        return (
            "not_recovered",
            json!(format!(
                "the catalog declares no answering question for {}, so it declares no \
                 repair a standing would run",
                standing.service
            )),
        );
    };
    let target = match crate::deploy::host_channel::canonical_target(&standing.host).await {
        Ok(target) => target,
        Err(error) => return ("not_recovered", json!(error.to_string())),
    };
    let Some(unit) = crate::deploy::service::declared_services(&target)
        .into_iter()
        .find(|declared| declared.name == standing.service)
        .map(|declared| declared.unit_id().to_string())
    else {
        return (
            "not_recovered",
            json!(format!(
                "the registry declares no {} unit on {}, so there is nothing to restart there",
                standing.service, standing.host
            )),
        );
    };
    let question = format!("{} {}", declared.method, declared.path);
    let earlier = match register::read(home) {
        Ok(records) => records.into_iter().rev().find(|record| {
            record.host == standing.host
                && record.service == standing.service
                && record.stood_since == standing.since
                && record.unit == unit
                && record.question == question
                && record.answering.as_ref() == Some(declared)
                && record.restarted
                && episode::repair_complete(&record.repair)
                && episode::repair_matches(&record.repair, &declared.repair)
        }),
        Err(error) => return ("not_recovered", json!(error.to_string())),
    };
    if let Some(record) = earlier {
        return ("already_recovered", json!(record));
    }
    if let Some(record) = episode::prior_peer(standing, &unit, declared) {
        return ("already_recovered", record);
    }
    let reading = Reading::Stood {
        since: standing.since.clone(),
        waited_seconds: standing.waited_seconds,
        bound_seconds: standing.bound_seconds,
    };
    let peer = Peer {
        observer: this_host,
        sentence: &standing.sentence,
    };
    match recover(home, &standing.host, &entry, &unit, &reading, Some(peer)).await {
        Ok(record) => {
            if record.get("recording_refusal").is_some()
                && record.get("restarted").and_then(Value::as_bool) == Some(true)
                && episode::repair_complete(&record["repair"])
            {
                episode::remember_peer(standing, &record);
            }
            ("recovery", record)
        }
        Err(error) => ("not_recovered", json!(error.to_string())),
    }
}
