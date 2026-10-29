//! What `stado network up` measures on this host and how it writes a node
//! into the registry document; what `down` takes back out.

use std::net::{IpAddr, Ipv4Addr};

use nix::net::if_::InterfaceFlags;
use serde_json::{json, Map, Value};

use crate::network::{network_name, NetworkNode};

/// The connection path a destination moved off the primary is filed under.
pub const LAN_PATH: &str = "lan";
pub const TAILSCALE_PATH: &str = "tailscale";
pub const DIRECT_PATH: &str = "direct";

/// What this host answers on, measured now.
#[derive(Debug, Clone, Default)]
pub struct Measured {
    /// `host:port` pairs, the multicast DNS name first.
    pub endpoints: Vec<String>,
    /// The name or address an SSH `lan` path names: the multicast DNS name
    /// when the link answers it with this host's address, else the first
    /// private address.
    pub lan: Option<String>,
}

/// RFC 6598 shared address space (100.64.0.0/10): carrier-grade NAT and
/// overlay networks hand it out, and it is never an address a peer on this
/// host's own network dials it at.
fn shared_space(address: Ipv4Addr) -> bool {
    let [first, second, ..] = address.octets();
    first == 100 && (64..128).contains(&second)
}

/// The IPv4 addresses of this host's interfaces that are up and running,
/// without loopback, link-local and shared-space ones.
pub fn interface_addresses() -> Result<Vec<Ipv4Addr>, String> {
    let mut found = Vec::new();
    for interface in nix::ifaddrs::getifaddrs()
        .map_err(|errno| format!("read this host's interfaces: {errno}"))?
    {
        let usable = interface
            .flags
            .contains(InterfaceFlags::IFF_UP | InterfaceFlags::IFF_RUNNING)
            && !interface.flags.contains(InterfaceFlags::IFF_LOOPBACK);
        let Some(address) = interface
            .address
            .as_ref()
            .and_then(|address| address.as_sockaddr_in())
            .map(|address| address.ip())
        else {
            continue;
        };
        if usable
            && !address.is_link_local()
            && !address.is_loopback()
            && !shared_space(address)
            && !found.contains(&address)
        {
            found.push(address);
        }
    }
    Ok(found)
}

/// Measure the endpoints a node listening on `port` answers on.
pub async fn measure(port: u16) -> Result<Measured, String> {
    let addresses = interface_addresses()?;
    let hostname = crate::providers::vast::system_hostname().to_ascii_lowercase();
    let short = hostname.split('.').next().filter(|name| !name.is_empty());
    let mut measured = Measured::default();
    if let Some(short) = short {
        let name = format!("{short}.local");
        // The name counts only when the link answers it with this host.
        if let Ok(answered) = crate::remote::lan::socket_addrs(&name, port).await {
            if answered.iter().any(
                |socket| matches!(socket.ip(), IpAddr::V4(address) if addresses.contains(&address)),
            ) {
                measured.endpoints.push(format!("{name}:{port}"));
                measured.lan = Some(name);
            }
        }
    }
    for address in &addresses {
        measured.endpoints.push(format!("{address}:{port}"));
        if measured.lan.is_none() && address.is_private() {
            measured.lan = Some(address.to_string());
        }
    }
    Ok(measured)
}

/// The path name a destination leaving the primary is filed under.
pub fn path_class(destination: &str) -> &'static str {
    let host = destination
        .rsplit_once('@')
        .map_or(destination, |(_, host)| host);
    let host = match host.strip_prefix('[').and_then(|rest| rest.split_once(']')) {
        Some((inner, _)) => inner,
        None => host,
    };
    match host.parse::<Ipv4Addr>() {
        Ok(address) if shared_space(address) => TAILSCALE_PATH,
        Ok(address) if address.is_private() => LAN_PATH,
        Ok(_) => DIRECT_PATH,
        Err(_) if host.to_ascii_lowercase().ends_with(".local") => LAN_PATH,
        Err(_) => DIRECT_PATH,
    }
}

fn target_entry<'a>(
    document: &'a mut Value,
    host: &str,
) -> Result<&'a mut Map<String, Value>, String> {
    document
        .get_mut("targets")
        .and_then(Value::as_array_mut)
        .and_then(|targets| {
            targets
                .iter_mut()
                .find(|target| target.get("name").and_then(Value::as_str) == Some(host))
        })
        .and_then(Value::as_object_mut)
        .ok_or_else(|| format!("the registry declares no host {host}"))
}

fn fallbacks(entry: &mut Map<String, Value>) -> Result<&mut Vec<Value>, String> {
    entry
        .entry("ssh_fallbacks")
        .or_insert_with(|| json!([]))
        .as_array_mut()
        .ok_or_else(|| "target.ssh_fallbacks must be an array".to_string())
}

/// What a declaration changed.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Declared {
    /// The network destination, now the primary path.
    pub primary: String,
    /// The destination the primary held before, and the path it moved to.
    pub moved: Option<(String, String)>,
    /// The `lan` path declared because the host had none.
    pub lan: Option<String>,
}

/// Write `node` as `host`'s network node and make its network destination
/// the primary path. The previous primary becomes the alternate its class
/// names, and a host with no `lan` path gets one.
pub fn declare(
    document: &mut Value,
    host: &str,
    node: &NetworkNode,
    lan: Option<&str>,
) -> Result<Declared, String> {
    let entry = target_entry(document, host)?;
    let previous = entry
        .get("ssh")
        .and_then(Value::as_str)
        .filter(|destination| !destination.trim().is_empty())
        .ok_or_else(|| format!("registry host {host} declares no SSH destination whose account the network path would log into; `stado registry host edit {host} --ssh USER@HOST` declares one"))?
        .to_string();
    let user = previous.split_once('@').map(|(user, _)| user).ok_or_else(|| {
        format!("registry host {host}'s SSH destination {previous} names no account; the network path needs USER@")
    })?;
    let primary = format!("{user}@{}", network_name(host));
    let lan_destination = lan.map(|lan| format!("{user}@{lan}"));
    entry.insert(
        "network".to_string(),
        serde_json::to_value(node).map_err(|error| error.to_string())?,
    );
    let mut declared = Declared {
        primary: primary.clone(),
        moved: None,
        lan: None,
    };
    let paths = fallbacks(entry)?;
    if previous != primary {
        let class = path_class(&previous);
        paths.retain(|path| {
            path.get("name").and_then(Value::as_str) != Some(class)
                && path.get("destination").and_then(Value::as_str) != Some(previous.as_str())
        });
        paths.insert(0, json!({"name": class, "destination": previous}));
        declared.moved = Some((previous, class.to_string()));
    }
    let has_lan = paths
        .iter()
        .any(|path| path.get("name").and_then(Value::as_str) == Some(LAN_PATH));
    if let (false, Some(destination)) = (has_lan, lan_destination) {
        paths.insert(0, json!({"name": LAN_PATH, "destination": destination}));
        declared.lan = Some(destination);
    }
    entry.insert("ssh".to_string(), json!(primary));
    Ok(declared)
}

/// Take `host`'s node out of the registry and make its first alternate path
/// the primary again. The answer is the destination now primary.
pub fn withdraw(document: &mut Value, host: &str) -> Result<Option<String>, String> {
    let entry = target_entry(document, host)?;
    let had_node = entry.remove("network").is_some();
    let on_network = entry
        .get("ssh")
        .and_then(Value::as_str)
        .is_some_and(|destination| {
            crate::network::network_host(
                destination
                    .rsplit_once('@')
                    .map_or(destination, |(_, host)| host),
            )
            .is_some()
        });
    if !on_network {
        return Ok(had_node
            .then(|| entry.get("ssh").and_then(Value::as_str).map(str::to_string))
            .flatten());
    }
    let paths = fallbacks(entry)?;
    if paths.is_empty() {
        return Err(format!(
            "registry host {host} has no SSH path but the Stado network, so taking the network down would leave it unreachable; `stado registry host path set {host} {LAN_PATH} --ssh USER@HOST` declares another first"
        ));
    }
    let next = paths.remove(0);
    let destination = next
        .get("destination")
        .and_then(Value::as_str)
        .ok_or_else(|| format!("registry host {host}'s first alternate path has no destination"))?
        .to_string();
    entry.insert("ssh".to_string(), json!(destination));
    if entry
        .get("ssh_fallbacks")
        .and_then(Value::as_array)
        .is_some_and(Vec::is_empty)
    {
        entry.remove("ssh_fallbacks");
    }
    Ok(Some(destination))
}
