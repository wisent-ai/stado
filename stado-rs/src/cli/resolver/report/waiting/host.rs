//! Whether this host's resolver already stands on a host, read before a
//! remote command is sent to it.
//!
//! A remote command rides the host channel to the host it names; the
//! resolver's adapters ride their own native SSH session to the same host
//! for every service placed there. When those connections stand, the
//! resolver records each wait. The command channel is a new connection,
//! and a session that stands does not make a new one stand: the host
//! answered a fresh SSH session at once while every relayed request on the
//! resolver's session stood for hours, and refusing the command left the
//! one diagnostic of the host unsent. So the reading is said beside the
//! command and names the host, its standing services, the endpoint and
//! since when; the command is refused only when no declared path to the
//! host answers either. The measure is the resolver's own: a wait longer
//! than the refresh interval the target declares, the same reading
//! `held_at` applies to one adapter.
//!
//! [`stood_elsewhere`] reads the same waits the other way round: not
//! whether a command to one host would stand, but which service on which
//! other host stands for this host's clients. A host judges its own
//! services by the question its catalog asks them; one that runs an older
//! catalog, or whose own Stado stands on the service it would judge, never
//! finds that service standing while every other host's resolver holds its
//! requests. A wait still opening its channel says the host or its SSH
//! daemon does not answer, not the service, and is not read there.

use std::collections::BTreeMap;

use super::{age_seconds, declared_refresh_seconds, published, unanswered, Phase, WaitingOpen};

/// What this host's resolver holds unanswered on `host` longer than its
/// refresh interval: every service of `host` that stands, with the held
/// wait's own sentence for the longest. `Ok(None)` when nothing to `host`
/// has waited that long; `Err` when the published list is there and cannot
/// be read, because then nobody can say.
pub(crate) fn held_on_host(host: &str) -> Result<Option<String>, String> {
    let waiting: Vec<(WaitingOpen, i64)> = published()?
        .into_iter()
        .filter(|open| open.active_host == host && open.session_replaced_at.is_none())
        .filter_map(|open| age_seconds(&open.since).map(|waited| (open, waited)))
        .collect();
    if waiting.is_empty() {
        return Ok(None);
    }
    let Some(refresh_seconds) = declared_refresh_seconds()? else {
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
        "this host's resolver holds {} connection(s) to {host} unanswered longer than the \
         {refresh_seconds}s refresh interval it declares (services {}): {}; `stado resolver \
         status` lists every one",
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
    let Some(refresh_seconds) = declared_refresh_seconds()? else {
        return Ok(None);
    };
    Ok(published()?
        .into_iter()
        .filter(|open| open.bind == bind && open.active_host == host && open.endpoint == endpoint)
        .filter(|open| open.session_replaced_at.is_none())
        .filter_map(|open| age_seconds(&open.since))
        .filter(|waited| *waited > refresh_seconds as i64)
        .min())
}

/// Why a request to `url` would wait without an answer, when this host's
/// resolver already holds a connection on that adapter unanswered longer
/// than its refresh interval: the held wait's own sentence, with how many
/// stand there. `Ok(None)` when `url` is not one of this host's adapter
/// binds, or nothing on that bind has waited that long; `Err` when the
/// published list is there and cannot be read, because then nobody can say.
/// The registry copy, which says the interval, is read only once a wait on
/// that bind is found: every store request asks here, and most binds hold
/// nothing.
pub(crate) fn held_at(url: &url::Url) -> Result<Option<String>, String> {
    let (Some(host), Some(port)) = (url.host_str(), url.port_or_known_default()) else {
        return Ok(None);
    };
    let bind = format!("{host}:{port}");
    let waiting: Vec<(WaitingOpen, i64)> = published()?
        .into_iter()
        .filter(|open| open.bind == bind && open.session_replaced_at.is_none())
        .filter_map(|open| age_seconds(&open.since).map(|waited| (open, waited)))
        .collect();
    if waiting.is_empty() {
        return Ok(None);
    }
    let Some(refresh_seconds) = declared_refresh_seconds()? else {
        return Ok(None);
    };
    let held: Vec<&(WaitingOpen, i64)> = waiting
        .iter()
        .filter(|(_, waited)| *waited > refresh_seconds as i64)
        .collect();
    let Some((longest, waited)) = held.iter().max_by_key(|(_, waited)| *waited) else {
        return Ok(None);
    };
    Ok(Some(format!(
        "this host's resolver holds {} connection(s) on that adapter unanswered longer than the \
         {refresh_seconds}s refresh interval it declares, so a request sent now would stand behind \
         them: {}; `stado resolver status` lists every one",
        held.len(),
        unanswered(longest, *waited)
    )))
}

/// One service on another host whose requests from this host stand.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Stood {
    pub(crate) service: String,
    /// The registry target the requests were relayed to.
    pub(crate) host: String,
    /// When the oldest standing request was accepted.
    pub(crate) since: String,
    /// How long the oldest standing request has waited.
    pub(crate) waited_seconds: u64,
    /// The refresh interval the waits are judged by.
    pub(crate) bound_seconds: u64,
    /// How many requests to the service stand.
    pub(crate) held: usize,
    /// The oldest wait's own sentence: client, request line, endpoint.
    pub(crate) sentence: String,
}

/// Every service on a host other than `this_host` that holds this host's
/// requests for the first byte of an answer longer than the declared
/// refresh interval, one entry per service and host, carrying its oldest
/// wait. Empty when no resolver here publishes waits, when none stands that
/// long, or when no refresh interval is declared (then nothing can be
/// judged standing); `Err` when the published list is there and cannot be
/// read.
pub(crate) fn stood_elsewhere(this_host: &str) -> Result<Vec<Stood>, String> {
    let answering: Vec<WaitingOpen> = published()?
        .into_iter()
        .filter(|open| {
            open.phase == Phase::Answer
                && open.active_host != this_host
                && open.session_replaced_at.is_none()
        })
        .collect();
    if answering.is_empty() {
        return Ok(Vec::new());
    }
    let Some(bound_seconds) = declared_refresh_seconds()? else {
        return Ok(Vec::new());
    };
    let mut routes: BTreeMap<(String, String), Vec<(WaitingOpen, i64)>> = BTreeMap::new();
    for open in answering {
        let Some(waited) = age_seconds(&open.since) else {
            continue;
        };
        if u64::try_from(waited).is_ok_and(|seconds| seconds > bound_seconds) {
            routes
                .entry((open.service.clone(), open.active_host.clone()))
                .or_default()
                .push((open, waited));
        }
    }
    Ok(routes
        .into_iter()
        .filter_map(|((service, host), waits)| {
            let held = waits.len();
            let (oldest, waited) = waits.into_iter().max_by_key(|(_, waited)| *waited)?;
            Some(Stood {
                service,
                host,
                since: oldest.since.clone(),
                waited_seconds: u64::try_from(waited).ok()?,
                bound_seconds,
                held,
                sentence: unanswered(&oldest, waited),
            })
        })
        .collect())
}
