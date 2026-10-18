//! A request already sent, waited on only for as long as this host's
//! resolver's own refresh interval.
//!
//! [`held_at`](super::held_at) and [`held_on_host`](super::host::held_on_host)
//! refuse a request before it is sent. A request sent before the route
//! stood — the registry read behind `stado resolver status`, an object API
//! request, the registry authority's snapshot over SSH — would otherwise
//! stand for as long as the route did. At every refresh interval — the one
//! this host's resolver publishes, or, with no resolver publishing one, the
//! one this machine's registry record declares for it — the published waits
//! are read again; once the route has gone unanswered longer than that
//! interval — this request's own connection among them — the request is
//! dropped, which closes its connection, and refused with the held wait's
//! sentence, naming the address or host it stood on. A request the far side
//! accepted and answers nothing on stands in no list of this host's, so a
//! request that outlasts the interval with nothing published held is
//! abandoned too, with the interval named and where to ask. Only a machine
//! whose record declares no resolver has no measure and waits. No other
//! bound exists: the one measure is the resolver's own.

use std::future::Future;
use std::time::Duration;

use super::declared_refresh_seconds;

/// What a request in flight rides: one of this host's adapter binds, the
/// channel to one host, or this host's own resolver API.
pub(crate) enum Route<'a> {
    Address(&'a url::Url),
    Host(&'a str),
    /// The resolver's own API, asked for the registry generation it holds.
    /// It is no adapter, so no published wait names a request to it.
    Resolver(&'a url::Url),
}

impl Route<'_> {
    fn held(&self) -> Result<Option<String>, String> {
        match self {
            Self::Address(url) => super::held_at(url),
            Self::Host(host) => super::host::held_on_host(host),
            Self::Resolver(_) => Ok(None),
        }
    }

    fn name(&self) -> String {
        match self {
            Self::Address(url) => url.to_string(),
            Self::Host(host) => host.to_string(),
            Self::Resolver(url) => url.to_string(),
        }
    }

    /// Whether a request on this route that outlasts the interval with
    /// nothing published held is abandoned at once. A request through an
    /// adapter is in the resolver's own list the moment its first byte goes
    /// up, and the next tick names it from there; the authority's snapshot
    /// over SSH and a request to the resolver's own API are in no list, so
    /// the interval is their only measure. A snapshot command that waited on
    /// its resolver without one outlived the SSH reader that started it, and
    /// hundreds of them held the authority's connections.
    fn unlisted(&self) -> bool {
        match self {
            Self::Address(_) => false,
            Self::Host(_) => true,
            Self::Resolver(_) => true,
        }
    }

    /// Where the answer to a request that outlasted the interval is: the
    /// far side's own account of what it waits on.
    fn ask(&self) -> String {
        match self {
            Self::Address(url) => format!(
                "`stado resolver status` lists what the adapter at {} holds and since when",
                url.authority()
            ),
            Self::Host(host) => format!(
                "the host's own `stado resolver snapshot` is what answered nothing: `stado host \
                 exec {host} -- stado waits --json` lists what every Stado process there waits \
                 on, and `stado service logs com.wisent.stado --host {host}` says what its unit \
                 is doing"
            ),
            Self::Resolver(url) => format!(
                "the resolver answering at {} accepted the request and answered nothing: \
                 `stado waits --json` lists what it waits on, and `stado service logs \
                 com.wisent.stado` says what its unit is doing",
                url.authority()
            ),
        }
    }
}

/// Wait for `sent` unless it outlasts the refresh interval this host's
/// resolver declares: `Ok(answer)` when the request answered or failed on
/// its own within it, `Err` with the sentence when it was abandoned — on a
/// route published held, with the held wait's own sentence; otherwise with
/// the interval it outlasted and where to ask, since a request the far side
/// accepted and answers nothing on stands in no list of this host's — or
/// when whether the route is held cannot be read, since then nobody can say:
/// the refresh interval that measures it included, which is a refusal before
/// the request is sent, not a request sent unwatched.
pub(crate) async fn unless_held<T, F>(route: Route<'_>, sent: F) -> Result<T, String>
where
    F: Future<Output = T>,
{
    let seconds = match declared_refresh_seconds() {
        Ok(Some(seconds)) => seconds,
        Ok(None) => return Ok(sent.await),
        Err(unreadable) => {
            return Err(format!(
                "{} request not sent: whether this host's resolver holds it cannot be measured: {unreadable}",
                route.name()
            ))
        }
    };
    let period = Duration::from_secs(seconds);
    tokio::pin!(sent);
    let mut schedule = tokio::time::interval_at(tokio::time::Instant::now() + period, period);
    loop {
        tokio::select! {
            answer = &mut sent => return Ok(answer),
            _ = schedule.tick() => match route.held() {
                Ok(None) if route.unlisted() => {
                    return Err(format!(
                        "{} request abandoned unanswered: it stood longer than the {seconds}s \
                         refresh interval this host's resolver declares; {}",
                        route.name(),
                        route.ask()
                    ))
                }
                Ok(None) => {}
                Ok(Some(held)) => {
                    return Err(format!(
                        "{} request abandoned unanswered: {held}",
                        route.name()
                    ))
                }
                Err(unreadable) => {
                    return Err(format!(
                        "{} request abandoned unanswered: whether this host's resolver holds \
                         that route cannot be read: {unreadable}",
                        route.name()
                    ))
                }
            },
        }
    }
}
