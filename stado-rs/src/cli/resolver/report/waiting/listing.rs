//! The published waits as `resolver status` lists them. A held route keeps
//! every new connection a client opens on it, so one route held for hours
//! publishes hundreds of waits; each stays in the JSON list for a machine to
//! read, and the blockers a person reads name each held route once, with
//! how many connections stand there and the oldest of them.

use std::collections::BTreeMap;

use serde_json::{json, Value};

use super::{age_seconds, published, unanswered, Phase, WaitingOpen};

/// The route a wait stands on: service, consumer, bind, phase, destination
/// and host. Waits that share it stand behind the same thing.
type Route = (String, String, String, Phase, String, String);

/// The published waits as JSON, every one, and one blocker for each route
/// whose waits have gone on longer than `refresh_seconds`. A list that
/// cannot be read is a blocker of its own, since nothing can then say
/// whether a route is held.
pub(in crate::cli::resolver::report) fn report(
    local: bool,
    refresh_seconds: u64,
) -> (Vec<Value>, Vec<String>) {
    let mut blockers = Vec::new();
    let published = match (local, published()) {
        (false, _) => Vec::new(),
        (true, Ok(published)) => published,
        (true, Err(unreadable)) => {
            blockers.push(unreadable);
            Vec::new()
        }
    };
    let mut listed = Vec::with_capacity(published.len());
    let mut held: BTreeMap<Route, Vec<(WaitingOpen, i64)>> = BTreeMap::new();
    for open in published {
        let waited = age_seconds(&open.since);
        listed.push(json!({
            "service": open.service,
            "consumer": open.consumer,
            "bind": open.bind,
            "active_host": open.active_host,
            "endpoint": open.endpoint,
            "since": open.since,
            "phase": open.phase,
            "client": open.client,
            "waited_seconds": waited,
        }));
        let Some(seconds) = waited.filter(|seconds| *seconds > refresh_seconds as i64) else {
            continue;
        };
        let route = (
            open.service.clone(),
            open.consumer.clone(),
            open.bind.clone(),
            open.phase,
            open.endpoint.clone(),
            open.active_host.clone(),
        );
        held.entry(route).or_default().push((open, seconds));
    }
    for waits in held.into_values() {
        // The published starts share one format, so the earliest sorts first.
        let Some((oldest, longest)) = waits
            .iter()
            .min_by(|(left, _), (right, _)| left.since.cmp(&right.since))
        else {
            continue;
        };
        let sentence = unanswered(oldest, *longest);
        blockers.push(match waits.as_slice() {
            [_] => sentence,
            _ => format!(
                "{} connections on one route, the oldest: {sentence}",
                waits.len()
            ),
        });
    }
    (listed, blockers)
}
