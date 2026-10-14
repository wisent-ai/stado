//! What a host does with a service that stands: the declared repair steps
//! of its repair declaration that the answering declaration names, in order,
//! then the declared restart through the same path `stado service restart`
//! takes, then the record and the alert. No manual step: the host that runs
//! the service acts on the beacon pass that found it standing, and the next
//! pass asks the new process afresh. Another host whose resolver holds the
//! service's requests past the same bound acts through this same path on
//! the host that runs it ([`Peer`]), since the steps are fixed host payloads
//! and the restart rides the registry SSH channel, neither of which needs
//! that host's own Stado.
//!
//! Every named step runs: each reads its own cause and refuses when that
//! cause is absent, so a standing caused by a wedged daemon is cleared by
//! the step for it whichever cause the declaration lists first. A recovery
//! that fails at a repair still restarts: the repairs clear the known
//! causes, the restart ends the process that holds the requests. A recovery
//! whose restart fails is recorded as such and alerted, and the beacon
//! carries the refusal, so a service that cannot be restarted does not read
//! as recovered.

use serde_json::{json, Value};

use super::register::{self, Recovery};
use super::Reading;
use crate::cli::CmdError;
use crate::deploy::service_catalog::CatalogService;

/// The alert's subject.
const SUBJECT: &str = "Stado recovered a service that stopped answering";

/// What another host saw stand: the host whose resolver held the service's
/// requests, and the oldest held request's own sentence.
#[derive(Debug, Clone, Copy)]
pub struct Peer<'a> {
    pub observer: &'a str,
    pub sentence: &'a str,
}

/// Recover `entry`, which `reading` found standing on `host`, and record
/// what was done. Answers the record as JSON for the beacon. `peer` is the
/// host that acts on a service of another host and what it saw; `None` when
/// the host that runs the service asked its own question. The caller that
/// asked the question forgets it afterwards ([`super::forget`]).
pub async fn recover(
    home: &std::path::Path,
    host: &str,
    entry: &CatalogService,
    unit: &str,
    reading: &Reading,
    peer: Option<Peer<'_>>,
) -> Result<Value, CmdError> {
    let Reading::Stood {
        since,
        waited_seconds,
        bound_seconds,
    } = reading
    else {
        return Err(CmdError::refused(format!(
            "{} does not stand ({}), so there is nothing to recover",
            entry.name,
            reading.document()
        )));
    };
    let declared = entry.answering.as_ref().ok_or_else(|| {
        CmdError::refused(format!(
            "{} declares no answering question, so nothing judged it standing",
            entry.name
        ))
    })?;
    let held_requests = match crate::cli::resolver::held_to_service(&entry.name) {
        Ok(held) => held,
        Err(cause) => vec![json!({ "unreadable": cause })],
    };
    let mut repair = Vec::with_capacity(declared.repair.len());
    for step in &declared.repair {
        repair.push(
            match crate::cli::repair::apply_step(&entry.name, step, host).await {
                Ok(report) => report,
                Err(error) => {
                    json!({ "name": step, "status": "refused", "refusal": error.to_string() })
                }
            },
        );
    }
    let (restart, restarted) = match crate::cli::service::lifecycle::release::unit::restart_quietly(
        &entry.name,
        Some(host),
    )
    .await
    {
        Ok(report) => (report, true),
        Err(error) => (json!({ "refusal": error.to_string() }), false),
    };
    let repaired = match repair.as_slice() {
        [] => String::from("not declared"),
        reports => reports
            .iter()
            .map(|report| format!("{} {}", report["name"], report["status"]))
            .collect::<Vec<_>>()
            .join(", "),
    };
    let restart_word = if restarted { "ended well" } else { "refused" };
    let message = match peer {
        None => format!(
            "{host}: {} ({unit}) answered nothing on {} {} for {waited_seconds}s, past the \
             {bound_seconds}s bound; {} request(s) stood on this host's resolver; repair \
             {repaired}; restart {restart_word}. `stado service recoveries` holds the record.",
            entry.name,
            declared.method,
            declared.path,
            held_requests.len(),
        ),
        Some(peer) => format!(
            "{host}: {} ({unit}) held {}'s requests for {waited_seconds}s, past the \
             {bound_seconds}s bound, and {host} did not recover it itself: {}; recovered from {} \
             over its registry SSH channel; repair {repaired}; restart {restart_word}. `stado \
             service recoveries` on {} holds the record.",
            entry.name, peer.observer, peer.sentence, peer.observer, peer.observer,
        ),
    };
    let alert: Vec<Value> = crate::monitor::alerts::send_alert(crate::config::alerts_topic(), &message, SUBJECT)
        .await
        .iter()
        .map(|delivery| {
            json!({ "channel": delivery.channel, "delivered": delivery.delivered, "detail": delivery.detail })
        })
        .collect();
    let recovery = Recovery {
        at: super::now_iso(),
        host: host.to_string(),
        service: entry.name.clone(),
        unit: unit.to_string(),
        question: format!("{} {}", declared.method, declared.path),
        stood_since: since.clone(),
        stood_seconds: *waited_seconds,
        bound_seconds: *bound_seconds,
        held_requests,
        repair: Value::Array(repair),
        restart,
        restarted,
        alert: Value::Array(alert),
        observed_by: peer.map(|peer| peer.observer.to_string()),
    };
    register::append(home, &recovery)?;
    Ok(json!(recovery))
}
