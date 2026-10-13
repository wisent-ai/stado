//! Waits left on a session the resolver replaced. When the native SSH
//! session one route rides stands while a new session to the same host
//! answers, new connections ride the new one and the held ones stay where
//! they are until their clients leave. Each of those is marked with when its
//! session was replaced, so `stado resolver status` says the route was
//! carried on and what the held connections still stand on, and a caller
//! reading the list before it sends is no longer refused on their account.
//!
//! The resolver asks this on every refresh tick ([`standing`]), not only when
//! a new connection arrives: a caller that reads the list refuses itself
//! before connecting, so a standing session would otherwise never meet the
//! connection that replaces it.

use std::collections::BTreeMap;

use super::{age_seconds, declared_refresh_seconds, now_iso, publish, WAITING};

/// One route this process holds unanswered longer than the refresh interval
/// on a session it has not replaced: who asked for what, where, and how long
/// the youngest of its waits has stood.
pub(crate) struct Standing {
    pub(crate) service: String,
    pub(crate) consumer: String,
    pub(crate) bind: String,
    pub(crate) active_host: String,
    pub(crate) endpoint: String,
    pub(crate) youngest_seconds: i64,
}

/// Every route this process holds unanswered past the declared refresh
/// interval whose waits ride a session not yet replaced; none without a
/// declared interval.
pub(crate) fn standing() -> Vec<Standing> {
    let Some(refresh_seconds) = declared_refresh_seconds() else {
        return Vec::new();
    };
    let opens = match WAITING.lock() {
        Ok(opens) => opens.clone(),
        Err(poisoned) => poisoned.into_inner().clone(),
    };
    let mut routes: BTreeMap<(String, String, String, String, String), i64> = BTreeMap::new();
    for open in opens.into_values() {
        let Some(waited) = age_seconds(&open.since) else {
            continue;
        };
        if waited <= refresh_seconds as i64 || open.session_replaced_at.is_some() {
            continue;
        }
        let route = (
            open.service,
            open.consumer,
            open.bind,
            open.active_host,
            open.endpoint,
        );
        let youngest = routes.entry(route).or_insert(waited);
        *youngest = (*youngest).min(waited);
    }
    routes
        .into_iter()
        .map(
            |((service, consumer, bind, active_host, endpoint), youngest_seconds)| Standing {
                service,
                consumer,
                bind,
                active_host,
                endpoint,
                youngest_seconds,
            },
        )
        .collect()
}

/// Mark every wait now on adapter `bind` to `endpoint` on `host` as riding
/// a session replaced at this instant, and publish the list.
pub(crate) fn replaced(bind: &str, host: &str, endpoint: &str) {
    let at = now_iso();
    let mut opens = match WAITING.lock() {
        Ok(opens) => opens,
        Err(poisoned) => poisoned.into_inner(),
    };
    opens
        .values_mut()
        .filter(|open| open.bind == bind && open.active_host == host && open.endpoint == endpoint)
        .filter(|open| open.session_replaced_at.is_none())
        .for_each(|open| open.session_replaced_at = Some(at.clone()));
    publish(&opens);
}
