//! A request already sent, waited on only for as long as this host's
//! resolver does not hold the route it rides.
//!
//! [`held_at`](super::held_at) and [`held_on_host`](super::host::held_on_host)
//! refuse a request before it is sent. A request sent before the route
//! stood — the registry read behind `stado resolver status`, an object API
//! request, the registry authority's snapshot over SSH — then stood for as
//! long as the route did: the command that diagnoses a held resolver stood
//! held for hours with the czekam line as its only word. At every refresh
//! interval this host's resolver declares, the published waits are read
//! again; once the route has gone unanswered longer than that interval —
//! this request's own connection among them — the request is dropped,
//! which closes its connection, and refused with the held wait's sentence,
//! naming the address or host it stood on. A route reached without a
//! resolver adapter, or a host that declares no resolver, has no such
//! reading and is waited on as before. No other bound exists: the one
//! measure is the resolver's own.

use std::future::Future;
use std::time::Duration;

use super::declared_refresh_seconds;

/// What a request in flight rides: one of this host's adapter binds, or the
/// channel to one host.
pub(crate) enum Route<'a> {
    Address(&'a url::Url),
    Host(&'a str),
}

impl Route<'_> {
    fn held(&self) -> Result<Option<String>, String> {
        match self {
            Self::Address(url) => super::held_at(url),
            Self::Host(host) => super::host::held_on_host(host),
        }
    }

    fn name(&self) -> String {
        match self {
            Self::Address(url) => url.to_string(),
            Self::Host(host) => host.to_string(),
        }
    }
}

/// Wait for `sent` unless the route it rides is published held first:
/// `Ok(answer)` when the request answered or failed on its own, `Err` with
/// the sentence when it was abandoned on a held route, or when whether the
/// route is held cannot be read, since then nobody can say.
pub(crate) async fn unless_held<T, F>(route: Route<'_>, sent: F) -> Result<T, String>
where
    F: Future<Output = T>,
{
    let Some(period) = declared_refresh_seconds().map(Duration::from_secs) else {
        return Ok(sent.await);
    };
    tokio::pin!(sent);
    let mut schedule = tokio::time::interval_at(tokio::time::Instant::now() + period, period);
    loop {
        tokio::select! {
            answer = &mut sent => return Ok(answer),
            _ = schedule.tick() => match route.held() {
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
