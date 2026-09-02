//! The tailnet's own name-to-address map, read from the local Tailscale node.
//!
//! Every Stado origin in this fleet is a MagicDNS name — the release channel is
//! `https://charless-mac-mini.tail6443b3.ts.net` — and reaching it depends on
//! the machine's resolver knowing that name. On 2026-09-02 the control-plane
//! runner's resolver for `tail6443b3.ts.net` listed `1.1.1.1` and `8.8.8.8`,
//! which cannot answer a MagicDNS name at all: the release origin resolved to
//! the public `ts.net` front end on one attempt and to nothing on the next,
//! while `100.100.100.100` answered `100.120.25.24` throughout and that address
//! served the release route in 82 ms. A release train read 20 MB of one
//! immutable object per 55 seconds and was cancelled at 55 minutes with the
//! object API healthy the whole time.
//!
//! Stado's own fleet channel never had that problem, because it reaches hosts
//! at the addresses the registry declares. This module gives the HTTP clients
//! the same footing: the tailnet states where its names live, so a tailnet
//! origin is pinned to the tailnet address and the system resolver is asked
//! only about names the tailnet does not own.
//!
//! The name is still what travels in SNI and in the certificate check. Pinning
//! decides the route, never the identity.

use std::collections::BTreeMap;
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::LazyLock;

use serde_json::Value;

/// Every suffix a tailnet MagicDNS name can end with.
const MAGICDNS_SUFFIX: &str = ".ts.net";

/// Where the Tailscale client keeps its CLI on the platforms this fleet runs.
/// The macOS App Store build ships it inside the bundle and puts nothing on
/// `PATH`, which is why the bundle path is listed rather than searched for.
const BINARIES: &[&str] = &[
    "/usr/bin/tailscale",
    "/usr/local/bin/tailscale",
    "/opt/homebrew/bin/tailscale",
    "/Applications/Tailscale.app/Contents/MacOS/Tailscale",
];

fn binary() -> Option<PathBuf> {
    BINARIES
        .iter()
        .map(PathBuf::from)
        .find(|path| path.is_file())
}

/// One `tailscale status --json` per process. The call costs about 0.2 s and
/// the tailnet's map does not change under a single command; a command that
/// asks no tailnet origin never pays it, because [`address_of`] rejects
/// non-tailnet names before this is read.
static MAP: LazyLock<BTreeMap<String, IpAddr>> =
    LazyLock::new(|| binary().and_then(|binary| read(&binary)).unwrap_or_default());

fn read(binary: &Path) -> Option<BTreeMap<String, IpAddr>> {
    let output = Command::new(binary).args(["status", "--json"]).output().ok()?;
    if !output.status.success() {
        return None;
    }
    let status: Value = serde_json::from_slice(&output.stdout).ok()?;
    let mut addresses = BTreeMap::new();
    record(status.get("Self"), &mut addresses);
    if let Some(peers) = status.get("Peer").and_then(Value::as_object) {
        for peer in peers.values() {
            record(Some(peer), &mut addresses);
        }
    }
    Some(addresses)
}

/// One node's MagicDNS name and the address the tailnet assigns it.
///
/// An offline peer is recorded like any other: a name the tailnet owns must
/// resolve to the tailnet, and a fast refusal from the right address beats a
/// public answer from the wrong one.
fn record(node: Option<&Value>, addresses: &mut BTreeMap<String, IpAddr>) {
    let Some(node) = node else { return };
    let Some(name) = node.get("DNSName").and_then(Value::as_str) else {
        return;
    };
    let name = name.trim_end_matches('.').to_ascii_lowercase();
    if !name.ends_with(MAGICDNS_SUFFIX) {
        return;
    }
    let Some(list) = node.get("TailscaleIPs").and_then(Value::as_array) else {
        return;
    };
    let parsed = list
        .iter()
        .filter_map(Value::as_str)
        .filter_map(|address| address.parse::<IpAddr>().ok());
    // IPv4 first: the CGNAT address is the one every registry host route in
    // this fleet is declared with, so pinning matches what `host exec` uses.
    let mut selected: Option<IpAddr> = None;
    for address in parsed {
        if address.is_ipv4() {
            selected = Some(address);
            break;
        }
        selected = selected.or(Some(address));
    }
    if let Some(address) = selected {
        addresses.insert(name, address);
    }
}

/// The address this machine's tailnet assigns to one MagicDNS name.
///
/// `None` for anything that is not a tailnet name, and for a tailnet name this
/// node has never seen. Both are cases where the system resolver is the only
/// witness there is.
pub(crate) fn address_of(host: &str) -> Option<IpAddr> {
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    if !host.ends_with(MAGICDNS_SUFFIX) {
        return None;
    }
    MAP.get(&host).copied()
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

#[cfg(test)]
mod tests {
    use super::*;

    fn status(body: &str) -> BTreeMap<String, IpAddr> {
        let value: Value = serde_json::from_str(body).expect("status fixture must parse");
        let mut addresses = BTreeMap::new();
        record(value.get("Self"), &mut addresses);
        if let Some(peers) = value.get("Peer").and_then(Value::as_object) {
            for peer in peers.values() {
                record(Some(peer), &mut addresses);
            }
        }
        addresses
    }

    #[test]
    fn the_map_keeps_the_tailnet_ipv4_address_of_every_node() {
        let addresses = status(
            r#"{
                "Self": {
                    "DNSName": "lukaszs-macbook-pro-4007-2.tail6443b3.ts.net.",
                    "TailscaleIPs": ["100.81.156.67", "fd7a:115c:a1e0::7334:9c43"]
                },
                "Peer": {
                    "nodekey:one": {
                        "DNSName": "charless-mac-mini.tail6443b3.ts.net.",
                        "TailscaleIPs": ["100.120.25.24", "fd7a:115c:a1e0::4b34:1918"]
                    }
                }
            }"#,
        );
        assert_eq!(
            addresses.get("charless-mac-mini.tail6443b3.ts.net"),
            Some(&"100.120.25.24".parse::<IpAddr>().expect("ipv4"))
        );
        assert_eq!(
            addresses.get("lukaszs-macbook-pro-4007-2.tail6443b3.ts.net"),
            Some(&"100.81.156.67".parse::<IpAddr>().expect("ipv4"))
        );
    }

    #[test]
    fn a_node_with_only_an_ipv6_address_is_still_recorded() {
        let addresses = status(
            r#"{
                "Self": {
                    "DNSName": "six.tail6443b3.ts.net.",
                    "TailscaleIPs": ["fd7a:115c:a1e0::1"]
                }
            }"#,
        );
        assert_eq!(
            addresses.get("six.tail6443b3.ts.net"),
            Some(&"fd7a:115c:a1e0::1".parse::<IpAddr>().expect("ipv6"))
        );
    }

    #[test]
    fn nodes_without_a_tailnet_name_or_address_are_omitted() {
        let addresses = status(
            r#"{
                "Self": {"DNSName": "", "TailscaleIPs": ["100.81.156.67"]},
                "Peer": {
                    "nodekey:none": {"DNSName": "elsewhere.example.com.", "TailscaleIPs": ["10.0.0.1"]},
                    "nodekey:empty": {"DNSName": "empty.tail6443b3.ts.net.", "TailscaleIPs": []}
                }
            }"#,
        );
        assert!(addresses.is_empty(), "{addresses:?}");
    }

    #[test]
    fn only_tailnet_names_are_pinned() {
        assert!(is_magicdns_name("charless-mac-mini.tail6443b3.ts.net."));
        assert!(!is_magicdns_name("stado.wisent.com"));
        assert_eq!(address_of("stado.wisent.com"), None);
    }
}
