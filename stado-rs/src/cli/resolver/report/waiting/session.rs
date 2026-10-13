//! Waits left on a session the resolver replaced. When the native SSH
//! session one route rides stands while a new session to the same host
//! answers, new connections ride the new one and the held ones stay where
//! they are until their clients leave. Each of those is marked with when its
//! session was replaced, so `stado resolver status` says the route was
//! carried on and what the held connections still stand on, instead of
//! reading as a route nobody can reach.

use super::{now_iso, publish, WAITING};

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
