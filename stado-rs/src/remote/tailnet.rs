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
use std::path::PathBuf;

use serde_json::Value;

mod reader;
mod status;
mod text;

use status::{command, decode};
use text::{names_tailnet, tailscale_assignable};

pub(crate) use reader::{client_state, end, read_beside};
pub(crate) use text::{
    destination_names_tailnet, destination_unreachable_because, is_magicdns_name,
};

/// Every suffix a tailnet MagicDNS name can end with.
const MAGICDNS_SUFFIX: &str = ".ts.net";

/// Where the Tailscale client keeps its CLI on the platforms this fleet runs,
/// when the host declares no paths of its own (`tailnet.clients`). The macOS
/// App Store build ships it inside the bundle and puts nothing on `PATH`,
/// which is why the bundle path is listed rather than searched for.
const BINARIES: &[&str] = &[
    "/usr/bin/tailscale",
    "/usr/local/bin/tailscale",
    "/opt/homebrew/bin/tailscale",
    "/Applications/Tailscale.app/Contents/MacOS/Tailscale",
];

/// The field that declares this host's own list of Tailscale client paths in
/// place of [`BINARIES`] (`STADO_TAILSCALE_CLIENTS`, `tailnet.clients`).
const CLIENTS: &crate::capabilities::ConfigField = &crate::capabilities::TAILNET_CLIENTS_CONFIG;

/// How much of what the Tailscale client printed instead of JSON a refusal
/// quotes: enough for its first sentence.
const PRINTED_CHARS: usize = 200;

/// The first declared Tailscale CLI that is a file; none when no declared path
/// holds one. A path that cannot be examined is the cause: skipped, the next
/// path, or no client at all, was taken for this machine's.
fn binary() -> Result<Option<PathBuf>, String> {
    let declared = crate::config_file::resolve_list(CLIENTS.env, CLIENTS.path, BINARIES);
    for path in declared.iter().map(PathBuf::from) {
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

/// One `tailscale status --json` per short-lived process. The call costs
/// about 0.2 s and the tailnet's map does not change under a single command;
/// a command that asks no tailnet origin never pays it, because
/// [`address_of`] rejects non-tailnet names before this is read. A
/// long-running process reads the map beside its roles instead
/// ([`read_beside`]), so a client that never answers holds none of them.
///
/// No Tailscale client is no map. A client that cannot be found or answer
/// leaves tailnet names to the system resolver too, but says so once:
/// silent, a crawl through the public `ts.net` front end looked like a slow
/// object API.
static MAP: std::sync::LazyLock<Tailnet> = std::sync::LazyLock::new(|| {
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
    match read_once(&binary) {
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

/// One `status --json` of the client at `binary`, run to its end.
fn read_once(binary: &std::path::Path) -> Result<Tailnet, String> {
    crate::wait::output(&mut command(binary))
        .map_err(|error| format!("`status --json` did not run: {error}"))
        .and_then(decode)
}

/// This node as its Tailscale client states it now, read afresh: how many
/// addresses the tailnet names, and why this node carries no traffic when it
/// carries none. `Ok(None)` when no Tailscale client is installed; `Err` names
/// the client and why it could not be read. What `stado host link` and the
/// release channel check read, through [`address_of`], asked directly.
pub fn own() -> Result<Option<(usize, Option<String>)>, String> {
    let Some(binary) = binary()? else {
        return Ok(None);
    };
    let read = read_once(&binary)
        .map_err(|error| format!("{} could not be read: {error}", binary.display()))?;
    Ok(Some((read.members.len(), read.own)))
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
    reader::with_map(|map| map.names.get(&host).copied())
}

/// Whether `address` is any address this machine's tailnet reports for one of
/// its nodes, including both IP families and nodes without MagicDNS names.
/// Membership is independent of the preferred address [`address_of`] pins.
/// An address range also admits carrier-grade NAT peers that share it, so the
/// range never decides membership: it decides only whether the tailnet is
/// asked at all, and an address outside it answers false without starting the
/// Tailscale client. No readable Tailscale status establishes no member.
pub(crate) fn is_node_address(address: IpAddr) -> bool {
    tailscale_assignable(address) && reader::with_map(|map| map.members.contains(&address))
}

/// The key `host` names in `map` (a tailnet address as text, or a MagicDNS
/// name), the name resolved in that same map.
fn key_in<'a>(map: &'a Tailnet, host: &str) -> Option<&'a NodeKey> {
    let name = host.trim_end_matches('.').to_ascii_lowercase();
    let named = map.names.get(&name).copied();
    map.keys
        .iter()
        .find(|(address, _)| address.to_string() == host || Some(**address) == named)
        .map(|(_, key)| key)
}

/// The key of the node `host` names (a tailnet address as text, or a MagicDNS
/// name), as this machine's tailnet states it; `None` for a host the tailnet
/// does not list, for a host that is not the tailnet's (read without asking
/// the client), or when no Tailscale status has been read.
pub(crate) fn node_key(host: &str) -> Option<NodeKey> {
    if !names_tailnet(host) {
        return None;
    }
    reader::with_map(|map| key_in(map, host).cloned())
}

/// Why this machine's tailnet cannot carry traffic to `host`, when the
/// tailnet itself says so: this machine's own node carries none (its
/// Tailscale not running or its own key expired), or the peer's key expired.
/// A connection through either only times out, and the expiry is the cause
/// the tailnet's admin renews, so a failure names it. Only a host the tailnet
/// lists, or a MagicDNS name, is judged; any other host is not the tailnet's,
/// and is answered from its text without asking the client.
pub(crate) fn unreachable_because(host: &str) -> Option<String> {
    if !names_tailnet(host) {
        return None;
    }
    reader::with_map(|map| {
        let key = key_in(map, host);
        if key.is_some() || is_magicdns_name(host) {
            if let Some(own) = &map.own {
                return Some(own.clone());
            }
        }
        key.and_then(NodeKey::cause)
    })
}
