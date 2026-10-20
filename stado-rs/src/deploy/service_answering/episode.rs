//! One successful restart per continuous failed answering episode. A later
//! successful API answer ends the episode; another failure can then recover.
//! An in-flight or recorded attempt is visible on the beacon instead of
//! repeatedly restarting a service that still refuses readiness. A refused
//! restart is retried on the next pass. An episode belongs to one API
//! question: a changed unit or endpoint does not inherit an old in-flight
//! wait, result or recovery.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};

use serde_json::{json, Value};

use super::{held, Finished, Reading};
use crate::deploy::service_catalog::AnsweringProbe;

/// The request a managed unit's answer belongs to across beacon passes.
pub(super) struct Question {
    unit: String,
    port: u16,
    method: String,
    path: String,
    body: Option<Value>,
}

impl Question {
    pub(super) fn new(unit: &str, port: u16, probe: &AnsweringProbe) -> Self {
        Self {
            unit: unit.to_owned(),
            port,
            method: probe.method.clone(),
            path: probe.path.clone(),
            body: probe.body.clone(),
        }
    }

    pub(super) fn matches(&self, unit: &str, port: u16, probe: &AnsweringProbe) -> bool {
        self.unit == unit
            && self.port == port
            && self.method == probe.method
            && self.path == probe.path
            && self.body == probe.body
    }
}

/// A finished answer, with the question it answered.
pub(super) struct Completed {
    pub question: Arc<Question>,
    pub result: Finished,
}

enum Attempt {
    Running,
    Recorded(Value),
}

struct Episode {
    question: Question,
    repair: Vec<String>,
    attempt: Attempt,
}

static ATTEMPTED: Mutex<BTreeMap<String, Episode>> = Mutex::new(BTreeMap::new());
/// Restarts performed from another host whose recovery register could not
/// append. Their actual outcome remains visible for the same held request
/// while this host runs; a successful register append is read from disk.
static UNRECORDED_PEERS: Mutex<BTreeMap<(String, String, String), Value>> =
    Mutex::new(BTreeMap::new());

pub(crate) fn observe(service: &str, reading: &Reading) {
    let successful = matches!(
        reading,
        Reading::Finished {
            result: Finished::Answered {
                successful: true,
                ..
            }
        } | Reading::Waiting {
            last: Some(Finished::Answered {
                successful: true,
                ..
            }),
            ..
        }
    );
    if successful {
        held(&ATTEMPTED).remove(service);
    }
}

/// A changed API question starts a different recovery episode, even on the
/// same unit and port.
pub(crate) fn forget(service: &str) {
    held(&ATTEMPTED).remove(service);
}

/// Reserve a failing episode before awaiting a repair. A different unit or
/// API question is a different service instance, even at the same port.
pub(crate) fn begin(service: &str, unit: &str, port: u16, probe: &AnsweringProbe) -> Option<Value> {
    let mut attempts = held(&ATTEMPTED);
    if let Some(episode) = attempts.get(service) {
        if episode.question.matches(unit, port, probe) && episode.repair == probe.repair {
            return Some(match &episode.attempt {
                Attempt::Running => json!({ "state": "already_attempting" }),
                Attempt::Recorded(record) => {
                    json!({ "state": "already_attempted", "last_recovery": record })
                }
            });
        }
    }
    attempts.insert(
        service.to_owned(),
        Episode {
            question: Question::new(unit, port, probe),
            repair: probe.repair.clone(),
            attempt: Attempt::Running,
        },
    );
    None
}

/// A restart alone does not complete the declared recovery when one of its
/// repair steps refused. Empty declarations are complete only with a
/// successful restart, as recorded by the caller.
pub(crate) fn repair_complete(reports: &Value) -> bool {
    reports.as_array().is_some_and(|steps| {
        steps
            .iter()
            .all(|step| step.get("status").and_then(Value::as_str) == Some("applied"))
    })
}

/// A completed attempt belongs to the current declaration only when every
/// repair report names the same step in the same order. An unchanged held
/// request does not make an earlier, different repair declaration complete.
pub(crate) fn repair_matches(reports: &Value, declared: &[String]) -> bool {
    reports.as_array().is_some_and(|steps| {
        steps.len() == declared.len()
            && steps
                .iter()
                .zip(declared)
                .all(|(step, name)| step.get("name").and_then(Value::as_str) == Some(name.as_str()))
    })
}

pub(crate) fn finish(service: &str, unit: &str, port: u16, probe: &AnsweringProbe, record: Value) {
    let mut attempts = held(&ATTEMPTED);
    let clear = match attempts.get_mut(service) {
        Some(episode)
            if episode.question.matches(unit, port, probe) && episode.repair == probe.repair =>
        {
            if record.get("restarted").and_then(Value::as_bool) == Some(true)
                && repair_complete(&record["repair"])
            {
                episode.attempt = Attempt::Recorded(record);
                false
            } else {
                // A refused repair or restart did not complete recovery; retry
                // on the next beacon after its dependency may have changed.
                true
            }
        }
        // A successful answer or a new service instance retired the old
        // reservation while its asynchronous recovery was running.
        _ => false,
    };
    if clear {
        attempts.remove(service);
    }
}

pub(crate) fn retain_declared(questions: &HashMap<String, bool>) {
    held(&ATTEMPTED).retain(|name, _| questions.contains_key(name));
}

pub(crate) fn prior_peer(
    standing: &crate::cli::resolver::Stood,
    unit: &str,
    probe: &AnsweringProbe,
) -> Option<Value> {
    let expected = json!(probe);
    held(&UNRECORDED_PEERS)
        .iter()
        .find_map(|((host, service, since), record)| {
            (host == &standing.host
                && service == &standing.service
                && since == &standing.since
                && record.get("unit").and_then(Value::as_str) == Some(unit)
                && record.get("answering") == Some(&expected)
                && record.get("restarted").and_then(Value::as_bool) == Some(true)
                && repair_complete(&record["repair"])
                && repair_matches(&record["repair"], &probe.repair))
            .then(|| record.clone())
        })
}

pub(crate) fn remember_peer(standing: &crate::cli::resolver::Stood, record: &Value) {
    held(&UNRECORDED_PEERS).insert(
        (
            standing.host.clone(),
            standing.service.clone(),
            standing.since.clone(),
        ),
        record.clone(),
    );
}

pub(crate) fn retain_peers(standing: &[crate::cli::resolver::Stood]) {
    held(&UNRECORDED_PEERS).retain(|(host, service, since), _| {
        standing.iter().any(|current| {
            &current.host == host && &current.service == service && &current.since == since
        })
    });
}
