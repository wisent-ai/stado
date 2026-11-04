//! One `tailscale status --json` of the client: the command every reader runs,
//! and its answer read into the tailnet map: every node's addresses, its
//! MagicDNS name and key, and why this machine's own node carries no traffic.

use std::net::IpAddr;
use std::path::Path;
use std::process::{Command, Stdio};

use serde_json::Value;

use super::{NodeKey, Tailnet, MAGICDNS_SUFFIX, PRINTED_CHARS};

/// The variable Tailscale's macOS app reads to answer as its command line.
/// Run with no terminal (any launchd unit, `stado serve` among them), the
/// program inside `Tailscale.app` takes itself for the app being opened and
/// starts its window instead: it printed "The Tailscale GUI failed to start"
/// or, with the app's window already up, never answered at all, and a
/// `stado serve` waiting on it at start never bound its API or resolver for
/// hours while launchd reported the unit running. The standalone
/// `tailscale` CLIs ignore it.
const BE_CLI: (&str, &str) = ("TAILSCALE_BE_CLI", "true");

/// `<binary> status --json`, answering as the client's command line and
/// reading no input: the one way every reader of the map runs the client.
pub(super) fn command(binary: &Path) -> Command {
    let mut command = Command::new(binary);
    command
        .args(["status", "--json"])
        .env(BE_CLI.0, BE_CLI.1)
        .stdin(Stdio::null());
    command
}

/// The map one `status --json` answer states, or why it states none.
pub(super) fn decode(output: std::process::Output) -> Result<Tailnet, String> {
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
            String::from_utf8_lossy(bytes)
                .trim()
                .chars()
                .take(PRINTED_CHARS)
                .collect()
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
        Some(other) => {
            return Err(format!(
                "`status --json` states BackendState as no text: {other}"
            ))
        }
    }
    let Some(own) = status.get("Self") else {
        return Ok(None);
    };
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
