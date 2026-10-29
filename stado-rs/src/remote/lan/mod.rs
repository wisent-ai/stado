//! A fleet host on this machine's LAN, named by its multicast DNS name
//! (`<name>.local`), resolved by Stado itself when this machine's resolver
//! does not speak multicast DNS.
//!
//! A macOS host answers `.local` names through its own resolver; a Linux
//! host without nss-mdns does not, so a registry path that names the
//! always-on Mac as `charles@<name>.local` failed on such a host with
//! "Temporary failure in name resolution" while both machines sat on one
//! LAN. With the tailnet path down too, that host's Stado could reach no
//! fleet store and published nothing. Asking the link itself (RFC 6762)
//! makes the declared LAN path work on every host, whatever its resolver.

use std::net::SocketAddr;
use std::time::Duration;

/// The domain RFC 6762 §3 reserves for link-local multicast DNS names.
const LINK_LOCAL_DOMAIN: &str = ".local";

/// How long one resolution waits for the link to answer. RFC 6762 §5.2
/// requires a querier to wait at least one second before its second query,
/// and §6 has a responder answer a unique record (a host's address) without
/// delay, so a host on the link has answered within that second or is not
/// answering the first query at all.
const FIRST_QUERY_INTERVAL: Duration = Duration::from_secs(1);

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

/// The addresses the link's multicast DNS responders give for `host`.
async fn multicast(host: &str) -> Result<Vec<std::net::IpAddr>, String> {
    let daemon =
        mdns_sd::ServiceDaemon::new().map_err(|error| format!("could not be asked: {error}"))?;
    let name = format!("{}.", host.trim_end_matches('.'));
    let window = u64::try_from(FIRST_QUERY_INTERVAL.as_millis()).map_err(|error| {
        format!("could not be asked: the answer window is no duration: {error}")
    })?;
    let events = daemon
        .resolve_hostname(&name, Some(window))
        .map_err(|error| format!("could not be asked: {error}"));
    let answer = match events {
        Ok(events) => loop {
            match events.recv_async().await {
                Ok(mdns_sd::HostnameResolutionEvent::AddressesFound(_, found)) => {
                    break Ok(found.iter().map(mdns_sd::ScopedIp::to_ip_addr).collect());
                }
                Ok(mdns_sd::HostnameResolutionEvent::SearchTimeout(_)) => {
                    break Err(format!(
                        "answered nothing within {}s (RFC 6762 §5.2's first query interval)",
                        FIRST_QUERY_INTERVAL.as_secs()
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
