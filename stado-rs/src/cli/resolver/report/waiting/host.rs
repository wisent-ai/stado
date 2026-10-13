//! Whether this host's resolver already stands on a host, read before a
//! remote command is sent to it.
//!
//! A remote command rides the host channel to the host it names; the
//! resolver's adapters ride channels to the same host for every service
//! placed there. When that host's services stand — accept a channel and
//! answer nothing — the resolver records each wait, and a command sent next
//! stands beside them: `stado host exec` and `stado service logs` to a host
//! whose vault stood held for minutes with no output, while the published
//! list already named that host and the connections it was not answering.
//! A command to such a host is refused before it is sent, with the host,
//! its standing services, the endpoint and since when it stands. The
//! measure is the resolver's own: a wait longer than the refresh interval
//! the target declares, the same reading `held_at` applies to one adapter.

use super::{age_seconds, declared_refresh_seconds, published, unanswered, WaitingOpen};

/// Why a command sent to `host` over its channel would stand without an
/// answer, when this host's resolver already holds connections to `host`
/// unanswered longer than its refresh interval: every service of `host`
/// that stands, with the held wait's own sentence for the longest.
/// `Ok(None)` when nothing to `host` has waited that long; `Err` when the
/// published list is there and cannot be read, because then nobody can say.
pub(crate) fn held_on_host(host: &str) -> Result<Option<String>, String> {
    let waiting: Vec<(WaitingOpen, i64)> = published()?
        .into_iter()
        .filter(|open| open.active_host == host)
        .filter_map(|open| age_seconds(&open.since).map(|waited| (open, waited)))
        .collect();
    if waiting.is_empty() {
        return Ok(None);
    }
    let Some(refresh_seconds) = declared_refresh_seconds() else {
        return Ok(None);
    };
    let held: Vec<&(WaitingOpen, i64)> = waiting
        .iter()
        .filter(|(_, waited)| *waited > refresh_seconds as i64)
        .collect();
    let Some((longest, waited)) = held.iter().max_by_key(|(_, waited)| *waited) else {
        return Ok(None);
    };
    let mut services: Vec<&str> = held.iter().map(|(open, _)| open.service.as_str()).collect();
    services.sort_unstable();
    services.dedup();
    Ok(Some(format!(
        "{host} is not sent this command: this host's resolver holds {} connection(s) to it \
         unanswered longer than the {refresh_seconds}s refresh interval it declares (services \
         {}), so a command sent now would stand with them: {}; `stado resolver status` lists \
         every one",
        held.len(),
        services.join(", "),
        unanswered(longest, *waited)
    )))
}

/// How many seconds the youngest of the waits on adapter `bind` to
/// `endpoint` on `host` has stood, among those unanswered longer than the
/// declared refresh interval: the resolver compares it with the age of the
/// SSH session it would send the next connection on, to tell whether the
/// held waits ride that session or one it already replaced. `Ok(None)` when
/// none stands that long; `Err` when the published list cannot be read.
pub(crate) fn youngest_held_seconds(
    bind: &str,
    host: &str,
    endpoint: &str,
) -> Result<Option<i64>, String> {
    let Some(refresh_seconds) = declared_refresh_seconds() else {
        return Ok(None);
    };
    Ok(published()?
        .into_iter()
        .filter(|open| open.bind == bind && open.active_host == host && open.endpoint == endpoint)
        .filter_map(|open| age_seconds(&open.since))
        .filter(|waited| *waited > refresh_seconds as i64)
        .min())
}
