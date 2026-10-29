//! A fleet host on this machine's LAN, named by its multicast DNS name
//! (`<name>.local`), resolved by Stado itself when this machine's resolver
//! does not speak multicast DNS.
//!
//! A macOS host answers `.local` names through its own resolver; a Linux
//! host without nss-mdns does not, so a registry path that names a host as
//! `<user>@<name>.local` fails there with "Temporary failure in name
//! resolution" even when both machines share one LAN. Asking the link itself
//! (RFC 6762) makes the declared LAN path work on every host, whatever its
//! resolver.

mod denied;

pub(crate) use denied::local_network;

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// The domain RFC 6762 §3 reserves for link-local multicast DNS names.
const LINK_LOCAL_DOMAIN: &str = ".local";

/// RFC 6762 §5.2: a querier waits at least one second before its second
/// query, and every later interval is at least twice the one before.
const FIRST_QUERY_INTERVAL: Duration = Duration::from_secs(1);

/// How long one resolution asks the link: the first query, its repetition
/// one interval later, and twice that interval for the repetition to be
/// answered (RFC 6762 §5.2), so 1 s + 2 s. A responder on a wireless link
/// misses single queries: one asked of a Mac on the LAN from a Linux host
/// went unanswered within its first second about six times in seven while
/// its unicast replies to other queries arrived. The window ends before a
/// five-second registry authority read would, with room for the SSH
/// handshake that follows the answer.
const ANSWER_WINDOW: Duration =
    Duration::from_secs(FIRST_QUERY_INTERVAL.as_secs() + 2 * FIRST_QUERY_INTERVAL.as_secs());

/// How long an answer stands: RFC 6762 §10's TTL for a record that names a
/// host's address. Every connection of a command to the same host (a
/// registry write, then the read that follows it) is answered from the first
/// answer instead of asking the link again.
const ANSWER_TTL: Duration = Duration::from_secs(120);

/// Each name's addresses, with when they stop standing.
type Answers = HashMap<String, (Vec<IpAddr>, Instant)>;

/// The answers this process was given.
static ANSWERED: Mutex<Option<Answers>> = Mutex::new(None);

/// The addresses this process was given for `name` that still stand.
fn answered(name: &str) -> Option<Vec<IpAddr>> {
    let held = ANSWERED
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let (addresses, until) = held.as_ref()?.get(name)?;
    (Instant::now() < *until).then(|| addresses.clone())
}

/// Keep `addresses` as the answer for `name` for its TTL.
fn keep(name: &str, addresses: &[IpAddr]) {
    let mut held = ANSWERED
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    held.get_or_insert_with(HashMap::new).insert(
        name.to_string(),
        (addresses.to_vec(), Instant::now() + ANSWER_TTL),
    );
}

/// Whether `host` is a multicast DNS name.
fn link_local(host: &str) -> bool {
    host.to_ascii_lowercase()
        .trim_end_matches('.')
        .ends_with(LINK_LOCAL_DOMAIN)
}

/// The addresses `host` names on `port`: this machine's resolver first, and
/// for a `.local` name it does not answer, the link's own multicast DNS
/// answer. A name neither answers is refused naming what each said.
pub(crate) async fn socket_addrs(host: &str, port: u16) -> Result<Vec<SocketAddr>, String> {
    let system = match tokio::net::lookup_host((host, port)).await {
        Ok(found) => {
            let found: Vec<SocketAddr> = found.collect();
            if !found.is_empty() {
                return Ok(found);
            }
            "this machine's resolver answered no address".to_string()
        }
        Err(error) => format!("this machine's resolver answered: {error}"),
    };
    if !link_local(host) {
        return Err(format!("{host} names no address: {system}"));
    }
    let waiting = crate::wait::begin(crate::wait::Kind::Network, "multicast DNS answer", host);
    match multicast(host).await {
        Ok(addresses) => {
            waiting.done();
            Ok(addresses
                .into_iter()
                .map(|address| SocketAddr::new(address, port))
                .collect())
        }
        Err(cause) => {
            waiting.failed(cause.clone());
            Err(format!(
                "{host} names no address: {system}, and the link's multicast DNS {cause}"
            ))
        }
    }
}

/// The addresses the link's multicast DNS responders give for `host`: the
/// answer this process already holds while it stands, else the link's.
async fn multicast(host: &str) -> Result<Vec<IpAddr>, String> {
    let name = format!("{}.", host.trim_end_matches('.').to_ascii_lowercase());
    if let Some(addresses) = answered(&name) {
        return Ok(addresses);
    }
    let daemon =
        mdns_sd::ServiceDaemon::new().map_err(|error| format!("could not be asked: {error}"))?;
    let window = u64::try_from(ANSWER_WINDOW.as_millis()).map_err(|error| {
        format!("could not be asked: the answer window is no duration: {error}")
    })?;
    let events = daemon
        .resolve_hostname(&name, Some(window))
        .map_err(|error| format!("could not be asked: {error}"));
    let answer = match events {
        Ok(events) => loop {
            match events.recv_async().await {
                Ok(mdns_sd::HostnameResolutionEvent::AddressesFound(_, found)) => {
                    let addresses: Vec<IpAddr> =
                        found.iter().map(mdns_sd::ScopedIp::to_ip_addr).collect();
                    keep(&name, &addresses);
                    break Ok(addresses);
                }
                Ok(mdns_sd::HostnameResolutionEvent::SearchTimeout(_)) => {
                    break Err(format!(
                        "answered nothing within {}s (two queries, RFC 6762 §5.2)",
                        ANSWER_WINDOW.as_secs()
                    ));
                }
                Ok(_) => {}
                Err(error) => break Err(format!("stopped answering: {error}")),
            }
        },
        Err(error) => Err(error),
    };
    // The daemon's thread ends with it; a shutdown it cannot send leaves
    // nothing behind that the answer depends on.
    if let Err(error) = daemon.shutdown() {
        tracing::debug!("the multicast DNS daemon for {host} did not take its shutdown: {error}");
    }
    answer
}
