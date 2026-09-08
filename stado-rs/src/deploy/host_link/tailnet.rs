//! Tailnet path: the path this host holds right now, and its own endpoint on
//! it, read out of `tailscale status --json`.

use serde_json::Value;

use super::probes::{probe, resolve_program};
use super::{PATH_KIND_DIRECT, PATH_KIND_RELAY, PATH_KIND_UNKNOWN};
use crate::deploy::Runner;

/// `(path_kind, endpoint)` from `tailscale status --json`, or `None` when
/// tailscale is absent or answered nothing parseable.
///
/// The host is reading about itself, and `Self.CurAddr` is empty on every
/// node (a node holds no path to itself), so directness is read from the
/// paths this host currently holds: a peer that is online with a `CurAddr` is
/// a direct path this host is party to. With no such peer, a node whose
/// backend is running still sits on its home DERP, which peers can reach it
/// through — that is a relay, not an absence.
///
/// `endpoint` stays about THIS host: its own dialable `ip:port` when direct
/// (the LAN spelling first, which is what one fleet on one network uses), and
/// `derp:<region>` when relayed. A peer's address is never published here as
/// if it were this host's.
pub(super) async fn tailnet_path(runner: &Runner) -> Option<(String, Option<String>)> {
    let program = resolve_program("tailscale")?;
    let output = probe(
        runner,
        vec![program, "status".to_string(), "--json".to_string()],
    )
    .await?;
    let status: Value = serde_json::from_str(&output.stdout).ok()?;

    // Tailscale answered, so the source is real from here on; only the path
    // can still be unknown.
    if status.get("BackendState").and_then(Value::as_str) != Some("Running") {
        return Some((PATH_KIND_UNKNOWN.to_string(), None));
    }
    let node = status.get("Self");
    let direct = status
        .get("Peer")
        .and_then(Value::as_object)
        .is_some_and(|peers| {
            peers.values().any(|peer| {
                peer.get("Online").and_then(Value::as_bool) == Some(true)
                    && peer
                        .get("CurAddr")
                        .and_then(Value::as_str)
                        .is_some_and(|address| !address.trim().is_empty())
            })
        });
    if direct {
        return Some((PATH_KIND_DIRECT.to_string(), self_endpoint(node)));
    }
    let relay = node
        .and_then(|node| node.get("Relay"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|region| !region.is_empty());
    match relay {
        Some(region) => Some((PATH_KIND_RELAY.to_string(), Some(format!("derp:{region}")))),
        None => Some((PATH_KIND_UNKNOWN.to_string(), None)),
    }
}

/// This host's own direct endpoint out of `Self.CurAddr`/`Self.Addrs`.
///
/// A private address wins over a public one: the fleet shares a network, the
/// operator's evidence for the silence that started this was the LAN spelling
/// (`direct 10.0.0.253:41641`), and that is the endpoint a peer on the same
/// network actually dials. IPv6 entries are skipped — every reader of this
/// field so far quotes the `ip:port` form.
fn self_endpoint(node: Option<&Value>) -> Option<String> {
    let node = node?;
    if let Some(current) = node
        .get("CurAddr")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|address| !address.is_empty())
    {
        return Some(current.to_string());
    }
    let addresses: Vec<&str> = node
        .get("Addrs")
        .and_then(Value::as_array)?
        .iter()
        .filter_map(Value::as_str)
        .map(str::trim)
        .filter(|address| !address.is_empty() && !address.starts_with('['))
        .collect();
    let private = addresses.iter().find(|address| {
        address
            .rsplit_once(':')
            .and_then(|(host, _)| host.parse::<std::net::Ipv4Addr>().ok())
            .is_some_and(|address| address.is_private())
    });
    private
        .or_else(|| addresses.first())
        .map(|address| (*address).to_string())
}
