//! The network's peer list, which is the registry's: each host's declared
//! node, read from this host's own copy of the registry so that reaching a
//! peer never first needs the registry authority the network may be what
//! reaches. A host or key the copy does not know asks the authority once,
//! and only one such read runs in a process at a time: the read itself may
//! travel over this network, and a lookup made inside it must not start
//! another.

use crate::targets::{ComputeTarget, Registry};

use super::NetworkNode;

/// Held while one authority read refreshes the copy.
static REFRESHING: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// The registry as this host holds it, without asking the authority.
pub async fn copy() -> Result<Registry, String> {
    crate::targets::registry_without_authority("the Stado network reads its peers")
        .await
        .map(|(registry, _)| registry)
        .map_err(|error| {
            format!("this host holds no registry copy to read its network peers from: {error}")
        })
}

/// The registry as the authority answers now, refreshing this host's copy;
/// `None` while another lookup in this process is already asking, or when
/// the authority does not answer.
pub async fn refreshed() -> Option<Registry> {
    let _held = REFRESHING.try_lock().ok()?;
    // Boxed: the authority may be read over SSH through this network, so
    // this future and the SSH connection's name each other.
    let read: futures::future::BoxFuture<'static, _> =
        Box::pin(crate::targets::fetch_registry_remote());
    read.await.ok()
}

/// The registry target named `host` and its declared node.
pub fn node_of<'a>(
    registry: &'a Registry,
    host: &str,
) -> Result<(&'a ComputeTarget, &'a NetworkNode), String> {
    let target = registry.lookup(host).ok_or_else(|| {
        format!("the registry declares no host {host}; `stado registry host path list {host}` names what it holds")
    })?;
    let node = target.network.as_ref().ok_or_else(|| {
        format!("registry host {host} declares no Stado network node; `stado network up` on {host} declares one")
    })?;
    Ok((target, node))
}

/// The host whose node holds this public key.
pub fn host_with_key(registry: &Registry, public: &[u8]) -> Option<String> {
    let encoded = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, public);
    registry
        .targets
        .iter()
        .find(|target| {
            target
                .network
                .as_ref()
                .is_some_and(|node| node.public_key == encoded)
        })
        .map(|target| target.name.clone())
}

/// `host`'s node from the copy, or from the authority when the copy does not
/// declare it.
pub async fn find(host: &str) -> Result<(ComputeTarget, NetworkNode), String> {
    let registry = copy().await?;
    match node_of(&registry, host) {
        Ok((target, node)) => Ok((target.clone(), node.clone())),
        Err(cause) => match refreshed().await {
            Some(fresh) => {
                node_of(&fresh, host).map(|(target, node)| (target.clone(), node.clone()))
            }
            None => Err(cause),
        },
    }
}

/// Every relay the copy declares other than `except`, with its target.
pub fn relays<'a>(
    registry: &'a Registry,
    except: &'a [&str],
) -> impl Iterator<Item = (&'a ComputeTarget, &'a NetworkNode)> {
    registry.targets.iter().filter_map(move |target| {
        let node = target.network.as_ref()?;
        (node.relay.is_some() && !except.contains(&target.name.as_str())).then_some((target, node))
    })
}

/// The SSH server ports `target`'s own registry paths name: the only ports
/// its node opens for a peer.
pub fn ssh_ports(target: &ComputeTarget) -> Vec<u16> {
    let mut ports: Vec<u16> = target
        .ssh_connections()
        .filter_map(|(_, destination)| destination_port(destination))
        .collect();
    ports.sort_unstable();
    ports.dedup();
    ports
}

/// The port an SSH destination dials: `[host]:port`, else SSH's own.
pub fn destination_port(destination: &str) -> Option<u16> {
    let host = destination
        .rsplit_once('@')
        .map_or(destination, |(_, host)| host);
    match host
        .strip_prefix('[')
        .and_then(|rest| rest.split_once("]:"))
    {
        Some((_, port)) => port.parse().ok(),
        None => Some(crate::deploy::host_access::native::SSH_PORT),
    }
}
