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

/// How much of what the Tailscale client printed instead of JSON a refusal
/// quotes: enough for its first sentence.
const PRINTED_CHARS: usize = 200;

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
    /// Why this machine's own node carries no traffic at all: its Tailscale
    /// is not `Running`, or its own key expired. A connection through such a
    /// node to any tailnet peer only times out, whatever the peer's key says.
    own: Option<String>,
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

/// This node as its Tailscale client states it now, read afresh: how many
/// addresses the tailnet names, and why this node carries no traffic when it
/// carries none. `Ok(None)` when no Tailscale client is installed; `Err` names
/// the client and why it could not be read. What `stado host link` and the
/// release channel check read, through [`address_of`], asked directly.
pub fn own() -> Result<Option<(usize, Option<String>)>, String> {
    let Some(binary) = binary()? else { return Ok(None) };
    let read = read(&binary).map_err(|error| format!("{} could not be read: {error}", binary.display()))?;
    Ok(Some((read.members.len(), read.own)))
}

/// The variable Tailscale's macOS app reads to answer as its command line.
/// Run with no terminal (any launchd unit, `stado serve` among them), the
/// program inside `Tailscale.app` takes itself for the app being opened and
/// starts its window instead: it printed "The Tailscale GUI failed to start"
/// or, with the app's window already up, never answered at all, and a
/// `stado serve` waiting on it at start never bound its API or resolver for
/// hours while launchd reported the unit running. The standalone
/// `tailscale` CLIs ignore it.
const BE_CLI: (&str, &str) = ("TAILSCALE_BE_CLI", "true");

fn read(binary: &Path) -> Result<Tailnet, String> {
    let output = crate::wait::output(
        Command::new(binary).args(["status", "--json"]).env(BE_CLI.0, BE_CLI.1).stdin(std::process::Stdio::null()),
    )
    .map_err(|error| format!("`status --json` did not run: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "`status --json` exited {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let status: Value = serde_json::from_slice(&output.stdout).map_err(|error| {
        // What the client printed instead is the cause (a client still
        // starting, or one asking for a login, answers in prose), so it is
        // named, cut to its first characters.
        let printed = |bytes: &[u8]| -> String {
            String::from_utf8_lossy(bytes).trim().chars().take(PRINTED_CHARS).collect()
        };
        format!(
            "`status --json` is not JSON: {error}; it printed {:?} on stdout and {:?} on stderr",
            printed(&output.stdout),
            printed(&output.stderr)
        )
    })?;
    let mut addresses = Tailnet::default();
    record(status.get("Self"), &mut addresses)?;
    addresses.own = own_cause(&status)?;
    match status.get("Peer") {
        None | Some(Value::Null) => {}
        Some(Value::Object(peers)) => {
            for peer in peers.values() {
                record(Some(peer), &mut addresses)?;
            }
        }
        Some(other) => {
            return Err(format!(
                "`status --json` lists its peers as no object: {other}"
            ))
        }
    }
    Ok(addresses)
}

/// Why this machine's own Tailscale carries no traffic, as its status says:
/// a `BackendState` other than `Running` (logged out, stopped, waiting for a
/// login), or the `Self` node's key expired.
fn own_cause(status: &Value) -> Result<Option<String>, String> {
    match status.get("BackendState") {
        None | Some(Value::Null) => {}
        Some(Value::String(state)) if state == "Running" => {}
        Some(Value::String(state)) => {
            return Ok(Some(format!(
                "this machine's Tailscale is {state}, not Running, so it carries no traffic"
            )))
        }
        Some(other) => return Err(format!("`status --json` states BackendState as no text: {other}")),
    }
    let Some(own) = status.get("Self") else { return Ok(None) };
    let key = NodeKey::of(own)?;
    if !key.expired {
        return Ok(None);
    }
    Ok(Some(match &key.expiry {
        Some(at) => format!("this machine's own Tailscale node key expired at {at}"),
        None => "this machine's own Tailscale node key expired; the status gives no KeyExpiry"
            .to_string(),
    }))
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
/// An address range also admits carrier-grade NAT peers that share it, so the
/// range never decides membership: it decides only whether the tailnet is
/// asked at all, and an address outside it answers false without starting the
/// Tailscale client. No readable Tailscale status establishes no member.
pub(crate) fn is_node_address(address: IpAddr) -> bool {
    tailscale_assignable(address) && MAP.members.contains(&address)
}

/// Whether Tailscale could have assigned `address` to a node: inside its
/// IPv4 range 100.64.0.0/10 or its IPv6 prefix fd7a:115c:a1e0::/48.
fn tailscale_assignable(address: IpAddr) -> bool {
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
fn names_tailnet(host: &str) -> bool {
    is_magicdns_name(host) || host.parse::<IpAddr>().is_ok_and(tailscale_assignable)
}

/// The key of the node `host` names (a tailnet address as text, or a MagicDNS
/// name), as this machine's tailnet states it; `None` for a host the tailnet
/// does not list, for a host that is not the tailnet's (read without asking
/// the client), or when no Tailscale status could be read.
pub(crate) fn node_key(host: &str) -> Option<&'static NodeKey> {
    if !names_tailnet(host) {
        return None;
    }
    let named = address_of(host);
    MAP.keys
        .iter()
        .find(|(address, _)| address.to_string() == host || Some(**address) == named)
        .map(|(_, key)| key)
}

/// Why this machine's tailnet cannot carry traffic to `host`, when the
/// tailnet itself says so: this machine's own node carries none (its
/// Tailscale not running or its own key expired), or the peer's key expired.
/// A connection through either only times out, and the expiry is the cause
/// the tailnet's admin renews, so a failure names it. Only a host the tailnet
/// lists, or a MagicDNS name, is judged; any other host is not the tailnet's.
pub(crate) fn unreachable_because(host: &str) -> Option<String> {
    let key = node_key(host);
    if key.is_some() || is_magicdns_name(host) {
        if let Some(own) = &MAP.own {
            return Some(own.clone());
        }
    }
    key.and_then(NodeKey::cause)
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

/// [`unreachable_because`] for an SSH destination: `user@host`,
/// `user@[host]:port` or a bare host.
pub(crate) fn destination_unreachable_because(destination: &str) -> Option<String> {
    unreachable_because(destination_host(destination))
}

/// Whether an SSH destination goes through a tailnet, read from its text
/// alone, so asking it never starts the Tailscale client.
pub(crate) fn destination_names_tailnet(destination: &str) -> bool {
    names_tailnet(destination_host(destination))
}
