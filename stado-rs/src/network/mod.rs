//! Stado's own host network: how one fleet host reaches another's SSH server
//! without any outside overlay.
//!
//! Every host whose Stado ran `stado network up` keeps a network node: a
//! listener on the port its system handed out, declared in the registry with
//! the host's public key and the endpoints it answers on. Two nodes speak the
//! Noise IK handshake (`snow`), each proving the static key the registry
//! declares for its host, and the responder opens one port of its own
//! loopback for the initiator: a port this host's registry SSH paths name.
//! Everything else Stado sends between hosts travels inside SSH, so carrying
//! SSH carries the resolver tunnel, the registry authority, the store and the
//! beacons with it.
//!
//! A host's address on this network is its registry name under
//! [`DOMAIN`]: `user@<host>.stado.internal` is an ordinary registry SSH
//! destination. Native SSH dials it through [`dial`]; OpenSSH through
//! `ProxyCommand` (`stado network connect`), which
//! [`crate::deploy::host_access::ssh_transport_options`] adds.
//!
//! A host behind NAT that no peer can dial attaches to a relay node, which
//! asks it to dial back when a peer wants it; the peer then runs a second,
//! end-to-end handshake with the host through the relay, which carries only
//! ciphertext.

use serde::{Deserialize, Serialize};

pub mod dial;
pub mod key;
pub mod node;
pub mod peers;
pub(crate) mod secure;

pub use dial::{dial, probe, Reached};

/// The names hosts carry on this network. `.internal` is the top-level
/// domain ICANN reserved for private use, so no public name can ever answer
/// for one of these.
pub const DOMAIN: &str = ".stado.internal";

/// The Noise protocol every node speaks: the IK handshake WireGuard is built
/// on, so an initiator that knows the responder's key from the registry
/// authenticates both sides in one round trip.
pub(crate) const NOISE_PARAMS: &str = "Noise_IK_25519_ChaChaPoly_BLAKE2s";

/// Bound into every handshake, so a session of another protocol or another
/// version of this one never completes against a node.
pub(crate) const PROLOGUE: &[u8] = b"stado-network/1";

/// What the registry declares for one host's node: `targets[].network`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NetworkNode {
    /// The node's X25519 public key, standard base64.
    pub public_key: String,
    /// `host:port` pairs a peer dials, tried together. Empty for a host
    /// reached only through a relay.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub endpoints: Vec<String>,
    /// Present when this node relays for hosts peers cannot dial.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub relay: Option<Relay>,
}

/// A relay's declaration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Relay {
    /// How long a node whose attachment to this relay ended waits before
    /// attaching again, as `stado network up --relay` was told.
    pub attach_retry_seconds: u64,
}

/// The registry host a network name addresses: `example-host.stado.internal`
/// names `example-host`. `None` for every name outside [`DOMAIN`].
pub fn network_host(host: &str) -> Option<&str> {
    let trimmed = host.trim_end_matches('.');
    let split = trimmed.len().checked_sub(DOMAIN.len())?;
    let (name, domain) = (trimmed.get(..split)?, trimmed.get(split..)?);
    (domain.eq_ignore_ascii_case(DOMAIN) && !name.is_empty()).then_some(name)
}

/// The network name of registry host `host`.
pub fn network_name(host: &str) -> String {
    format!("{host}{DOMAIN}")
}

/// The JSON request a connection opens with, after the handshake.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum Request {
    /// Answer with this node's host name and open nothing.
    Probe,
    /// Open this port on the responder's loopback.
    Open { port: u16 },
    /// Carry this connection to another host's node (relay only).
    Relay { host: String },
    /// Keep this connection as the sender's standing attachment (relay only).
    Attach,
    /// The dial-back a relay asked an attached host for.
    Accept { token: String },
    /// Open `port` on registry host `host`'s loopback for a process of the
    /// responder's own host: the node makes the connection, so it leaves
    /// the host as the node's program, under the Local Network permission
    /// macOS keeps for it across installs.
    Reach { host: String, port: u16 },
}

/// The responder's one answer before data flows.
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum Answer {
    Ok {
        host: String,
    },
    Refused {
        reason: String,
    },
    /// On an attachment: dial back to the relay with this token.
    Dial {
        token: String,
    },
}
