//! Stado-network path: the path this host holds right now over its own
//! node, read the way `stado network status` reads it.

use super::{PATH_KIND_DIRECT, PATH_KIND_RELAY};
use crate::network::node::{serving, State};

/// `(path_kind, endpoint)` of this host's Stado network node: none when this
/// host never joined the network; an error naming why when its node does not
/// serve, or when no declared peer answered. Every declared peer is asked at
/// once, as `stado network status` asks them, so one silent peer does not
/// hold the answer.
///
/// `direct` when a peer answered over one of its declared endpoints, and
/// `relay` when the only answers came through a relay; `endpoint` is this
/// host's own `host:port` on the network: the first endpoint the registry
/// declares for it, which is what a peer dials.
pub(super) async fn own_network_path() -> Result<Option<(String, Option<String>)>, String> {
    let Some(state) = State::load()? else {
        return Ok(None);
    };
    let published = serving::read()?
        .ok_or_else(|| format!("{}'s Stado network node has published nothing", state.host))?;
    let serving = published.state == serving::SERVING
        && published.port == state.port
        && crate::providers::local::helpers::pid_alive(published.pid as i32);
    if !serving {
        return Err(format!(
            "{}'s Stado network node is not serving: {}",
            state.host,
            published
                .reason
                .as_deref()
                .unwrap_or(published.state.as_str())
        ));
    }
    let registry = crate::network::peers::copy().await?;
    let own_endpoint = registry
        .targets
        .iter()
        .find(|target| target.name == state.host)
        .and_then(|target| target.network.as_ref())
        .and_then(|node| node.endpoints.first().cloned());
    let peers: Vec<String> = registry
        .targets
        .iter()
        .filter(|target| target.name != state.host && target.network.is_some())
        .map(|target| target.name.clone())
        .collect();
    if peers.is_empty() {
        return Err(format!(
            "the registry declares no other host on the Stado network, so {} has no peer to hold a path to",
            state.host
        ));
    }
    let answers =
        futures::future::join_all(peers.iter().map(|peer| crate::network::probe(peer))).await;
    let mut relayed = false;
    let mut causes = Vec::new();
    for (peer, answer) in peers.iter().zip(answers) {
        match answer {
            Ok(reached) if reached.path == "direct" => {
                return Ok(Some((PATH_KIND_DIRECT.to_string(), own_endpoint)));
            }
            Ok(_) => relayed = true,
            Err(cause) => causes.push(format!("{peer}: {cause}")),
        }
    }
    if relayed {
        return Ok(Some((PATH_KIND_RELAY.to_string(), own_endpoint)));
    }
    Err(format!(
        "no Stado network peer answered ({})",
        causes.join("; ")
    ))
}
