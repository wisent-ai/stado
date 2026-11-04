//! What a host's text alone says about the tailnet: whether it is a MagicDNS
//! name or an address Tailscale assigns, and which host an SSH destination
//! names. None of these asks the Tailscale client.

use std::net::IpAddr;

use super::MAGICDNS_SUFFIX;

/// Whether Tailscale could have assigned `address` to a node: inside its
/// IPv4 range 100.64.0.0/10 or its IPv6 prefix fd7a:115c:a1e0::/48.
pub(super) fn tailscale_assignable(address: IpAddr) -> bool {
    match address {
        IpAddr::V4(v4) => {
            let [first, second, ..] = v4.octets();
            first == 100 && (64..128).contains(&second)
        }
        IpAddr::V6(v6) => {
            let segments = v6.segments();
            segments[0] == 0xfd7a && segments[1] == 0x115c && segments[2] == 0xa1e0
        }
    }
}

/// Whether `host` (a name or an address as text) can only be reached through
/// a tailnet: a MagicDNS name, or an address Tailscale assigns. Decided from
/// the text alone, so asking it never starts the Tailscale client. Tailscale
/// is an optional route: a host on the fleet's own network, a LAN name or any
/// other address is never judged by the tailnet.
pub(super) fn names_tailnet(host: &str) -> bool {
    is_magicdns_name(host) || host.parse::<IpAddr>().is_ok_and(tailscale_assignable)
}

/// Whether one host name belongs to a tailnet, judged by its suffix alone.
///
/// Callers that report on resolution need to separate "the tailnet owns this
/// name and could not be asked" from "this name was never a tailnet name".
pub(crate) fn is_magicdns_name(host: &str) -> bool {
    host.trim_end_matches('.')
        .to_ascii_lowercase()
        .ends_with(MAGICDNS_SUFFIX)
}

/// The host an SSH destination names: `user@host`, `user@[host]:port` or a
/// bare host.
fn destination_host(destination: &str) -> &str {
    let host = match destination.rsplit_once('@') {
        Some((_, host)) => host,
        None => destination,
    };
    match host.strip_prefix('[') {
        Some(bracketed) => match bracketed.split_once(']') {
            Some((inner, _)) => inner,
            None => bracketed,
        },
        None => host,
    }
}

/// [`super::unreachable_because`] for an SSH destination: `user@host`,
/// `user@[host]:port` or a bare host.
pub(crate) fn destination_unreachable_because(destination: &str) -> Option<String> {
    super::unreachable_because(destination_host(destination))
}

/// Whether an SSH destination goes through a tailnet, read from its text
/// alone, so asking it never starts the Tailscale client.
pub(crate) fn destination_names_tailnet(destination: &str) -> bool {
    names_tailnet(destination_host(destination))
}
