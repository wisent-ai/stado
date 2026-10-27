//! The tailnet's own name-to-address map, read from the local Tailscale node.
//!
//! Every Stado origin in this fleet is a MagicDNS name —
//! `https://<host>.<tailnet>.ts.net` — and reaching it depends on the
//! machine's resolver knowing that name. A runner whose resolver for the
//! tailnet domain lists only public resolvers cannot answer a MagicDNS name
//! at all: the origin resolves to the public `ts.net` front end on one attempt
//! and to nothing on the next, while the tailnet's own resolver answers the
//! tailnet address throughout and that address serves the route quickly. A
//! release train then crawls through one immutable object and is cancelled
//! with the object API healthy the whole time.
//!
//! Stado's own fleet channel never had that problem, because it reaches hosts
//! at the addresses the registry declares. This module gives the HTTP clients
//! the same footing: the tailnet states where its names live, so a tailnet
//! origin is pinned to the tailnet address and the system resolver is asked
//! only about names the tailnet does not own.
//!
//! The name is still what travels in SNI and in the certificate check. Pinning
//! decides the route, never the identity.

use std::collections::{BTreeMap, BTreeSet};
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

/// The first listed Tailscale CLI that is a file; none when no listed path
/// holds one. A listed path that cannot be examined is the cause: skipped,
/// the next path, or no client at all, was taken for this machine's.
fn binary() -> Result<Option<PathBuf>, String> {
    for path in BINARIES.iter().map(PathBuf::from) {
        match std::fs::metadata(&path) {
            Ok(metadata) if metadata.is_file() => return Ok(Some(path)),
            Ok(_not_file) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(format!("{} cannot be examined: {error}", path.display())),
        }
    }
    Ok(None)
}

/// DNS pinning selects one address, but transport membership includes every
/// address Tailscale reports, whether or not the node has a MagicDNS name.
#[derive(Default)]
struct Tailnet {
    names: BTreeMap<String, IpAddr>,
    members: BTreeSet<IpAddr>,
    /// Each node's key as the tailnet states it, by every address of the
    /// node: whether it expired (`Expired`) and when it does (`KeyExpiry`).
    keys: BTreeMap<IpAddr, NodeKey>,
}

/// One node's key as `tailscale status --json` states it. `expiry` is absent
/// for a node whose key expiry the tailnet's admin turned off.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NodeKey {
    pub(crate) expired: bool,
    pub(crate) expiry: Option<String>,
}

impl NodeKey {
    /// Read one node's key fields; a field of another type than the status
    /// declares is refused with the node named.
    fn of(node: &Value) -> Result<Self, String> {
        let expired = match node.get("Expired") {
            None | Some(Value::Null) => false,
            Some(Value::Bool(expired)) => *expired,
            Some(other) => return Err(format!("{node} states Expired as no boolean: {other}")),
        };
        let expiry = match node.get("KeyExpiry") {
            None | Some(Value::Null) => None,
            Some(Value::String(at)) => Some(at.clone()),
            Some(other) => return Err(format!("{node} states KeyExpiry as no text: {other}")),
        };
        Ok(Self { expired, expiry })
    }

    /// Why the tailnet carries no traffic to this node, when its key expired.
    pub(crate) fn cause(&self) -> Option<String> {
        if !self.expired {
            return None;
        }
        Some(match &self.expiry {
            Some(at) => format!("its Tailscale node key expired at {at}"),
            None => "its Tailscale node key expired; the status gives no KeyExpiry".to_string(),
        })
    }
}

/// One `tailscale status --json` per process. The call costs about 0.2 s and
/// the tailnet's map does not change under a single command; a command that
/// asks no tailnet origin never pays it, because [`address_of`] rejects
/// non-tailnet names before this is read.
///
/// No Tailscale client is no map. A client that cannot be found or answer
/// leaves tailnet names to the system resolver too, but says so once:
/// silent, a crawl through the public `ts.net` front end looked like a slow
/// object API.
static MAP: LazyLock<Tailnet> = LazyLock::new(|| {
    let binary = match binary() {
        Ok(Some(binary)) => binary,
        Ok(None) => return Tailnet::default(),
        Err(unexamined) => {
            eprintln!(
                "[tailnet] the Tailscale client cannot be located, so MagicDNS names are left to \
                 the system resolver: {unexamined}"
            );
            return Tailnet::default();
        }
    };
    match read(&binary) {
        Ok(addresses) => addresses,
        Err(error) => {
            eprintln!(
                "[tailnet] {} could not be read, so MagicDNS names are left to the system \
                 resolver: {error}",
                binary.display()
            );
            Tailnet::default()
        }
    }
});

fn read(binary: &Path) -> Result<Tailnet, String> {
    let output = crate::wait::output(Command::new(binary).args(["status", "--json"]))
        .map_err(|error| format!("`status --json` did not run: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "`status --json` exited {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let status: Value = serde_json::from_slice(&output.stdout)
        .map_err(|error| format!("`status --json` is not JSON: {error}"))?;
    let mut addresses = Tailnet::default();
    record(status.get("Self"), &mut addresses)?;
    match status.get("Peer") {
        None | Some(Value::Null) => {}
        Some(Value::Object(peers)) => {
            for peer in peers.values() {
                record(Some(peer), &mut addresses)?;
            }
        }
        Some(other) => return Err(format!("`status --json` lists its peers as no object: {other}")),
    }
    Ok(addresses)
}

/// One node's MagicDNS name and the address the tailnet assigns it.
///
/// An offline peer is recorded like any other: a name the tailnet owns must
/// resolve to the tailnet, and a fast refusal from the right address beats a
/// public answer from the wrong one.
///
/// A tailnet node whose listed address is not text or not an address is a
/// status this reader does not understand, and is refused with the node
/// named: skipping it would hand that name to the public resolver unseen.
fn record(node: Option<&Value>, addresses: &mut Tailnet) -> Result<(), String> {
    let Some(node) = node else { return Ok(()) };
    let Some(list) = node.get("TailscaleIPs").and_then(Value::as_array) else {
        return Ok(());
    };
    let mut selected: Option<IpAddr> = None;
    let key = NodeKey::of(node)?;
    for listed in list {
        let address = listed
            .as_str()
            .ok_or_else(|| format!("{node} lists a tailnet address that is not text: {listed}"))?;
        let address = address.parse::<IpAddr>().map_err(|error| {
            format!("{node} lists the tailnet address {address:?}, which is no address: {error}")
        })?;
        addresses.members.insert(address);
        addresses.keys.insert(address, key.clone());
        // Keep the existing IPv4-first DNS pin without dropping the other
        // family from the separately observed transport membership.
        selected = match selected {
            None => Some(address),
            Some(current) if current.is_ipv6() && address.is_ipv4() => Some(address),
            previous => previous,
        };
    }
    if let (Some(name), Some(address)) = (node.get("DNSName").and_then(Value::as_str), selected) {
        let name = name.trim_end_matches('.').to_ascii_lowercase();
        if name.ends_with(MAGICDNS_SUFFIX) {
            addresses.names.insert(name, address);
        }
    }
    Ok(())
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
    MAP.names.get(&host).copied()
}

/// Whether `address` is any address this machine's tailnet reports for one of
/// its nodes, including both IP families and nodes without MagicDNS names.
/// Membership is independent of the preferred address [`address_of`] pins.
/// An address range also admits carrier-grade NAT peers that share it. No
/// readable Tailscale status establishes no member.
pub(crate) fn is_node_address(address: IpAddr) -> bool {
    MAP.members.contains(&address)
}

/// The key of the node `host` names (a tailnet address as text, or a MagicDNS
/// name), as this machine's tailnet states it; `None` for a host the tailnet
/// does not list or when no Tailscale status could be read.
pub(crate) fn node_key(host: &str) -> Option<&'static NodeKey> {
    let named = address_of(host);
    MAP.keys
        .iter()
        .find(|(address, _)| address.to_string() == host || Some(**address) == named)
        .map(|(_, key)| key)
}

/// Why this machine's tailnet cannot carry traffic to `host`, when the
/// tailnet itself says so: the node's key expired. A connection to such a
/// node only times out, and the expiry is the cause the tailnet's admin
/// renews, so a failure names it.
pub(crate) fn unreachable_because(host: &str) -> Option<String> {
    node_key(host).and_then(NodeKey::cause)
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

/// [`unreachable_because`] for an SSH destination: `user@host`,
/// `user@[host]:port` or a bare host.
pub(crate) fn destination_unreachable_because(destination: &str) -> Option<String> {
    let host = match destination.rsplit_once('@') {
        Some((_, host)) => host,
        None => destination,
    };
    let host = match host.strip_prefix('[') {
        Some(bracketed) => match bracketed.split_once(']') {
            Some((inner, _)) => inner,
            None => bracketed,
        },
        None => host,
    };
    unreachable_because(host)
}
