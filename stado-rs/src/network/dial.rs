//! Reaching a peer's node: every endpoint the registry declares for it,
//! every relay the registry declares and, for a connection a process of this
//! host opens, this host's own node are tried together, and the first that
//! completes the handshake and is answered carries the connection. A failure
//! names each way that was tried and what it answered.

use futures::future::BoxFuture;
use futures::stream::{FuturesUnordered, StreamExt};
use tokio::io::DuplexStream;
use tokio::net::TcpStream;

use super::key::{self, Keypair};
use super::secure::{self, Stream};
use super::{peers, Answer, NetworkNode, Request};

/// How a peer was reached, for receipts.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Reached {
    /// The registry host that answered.
    pub host: String,
    /// `direct`, `relay` or `node` (this host's own node opened it).
    pub path: &'static str,
    /// The endpoint dialled, the relay host, or this host for its node.
    pub through: String,
}

/// A plaintext stream to port `port` of registry host `host`'s loopback.
///
/// A named boxed future, not an `async fn`: reaching a peer may read the
/// registry, whose authority is itself reached over SSH through this
/// function, so the SSH connection's future and this one would otherwise
/// each need the other's type to be known first.
pub fn dial(host: &str, port: u16) -> BoxFuture<'static, Result<DuplexStream, String>> {
    let host = host.to_string();
    Box::pin(async move {
        let place = format!("{}:{port}", super::network_name(&host));
        let waiting = crate::wait::begin(
            crate::wait::Kind::Network,
            "Stado network connection",
            &place,
        );
        match connect(&host, Request::Open { port }, true).await {
            Ok((stream, _)) => {
                waiting.done();
                Ok(stream)
            }
            Err(cause) => {
                waiting.failed(&cause);
                Err(cause)
            }
        }
    })
}

/// Reach `host`'s node and have it answer, opening nothing.
pub async fn probe(host: &str) -> Result<Reached, String> {
    connect(host, Request::Probe, false)
        .await
        .map(|(_, reached)| reached)
}

/// [`dial`] by this process alone, for this host's node when it opens a
/// connection a process of its host asked for: asking itself again would
/// never end.
pub(crate) async fn direct(host: &str, port: u16) -> Result<DuplexStream, String> {
    connect(host, Request::Open { port }, false)
        .await
        .map(|(stream, _)| stream)
}

type Attempt = BoxFuture<'static, Result<(DuplexStream, Reached), String>>;

/// `through_this_node` adds the way through this host's own node, whose
/// program keeps its macOS Local Network permission across installs while
/// this process's program does not.
async fn connect(
    host: &str,
    request: Request,
    through_this_node: bool,
) -> Result<(DuplexStream, Reached), String> {
    let keypair = key::load()?;
    let (_, node) = peers::find(host).await?;
    let peer = key::decode_public(&node.public_key)
        .map_err(|cause| format!("registry host {host}: {cause}"))?;
    let registry = peers::copy().await?;
    let own = keypair.public_base64();
    let mut attempts: Vec<(String, Attempt)> = Vec::new();
    for endpoint in &node.endpoints {
        let (keypair, peer, request, endpoint) = (
            keypair.clone(),
            peer.clone(),
            request.clone(),
            endpoint.clone(),
        );
        let host = host.to_string();
        attempts.push((
            format!("endpoint {endpoint}"),
            Box::pin(async move {
                let tcp = tcp(&endpoint).await?;
                let (stream, answered) = open(tcp, &keypair, &peer, &request).await?;
                Ok((
                    stream,
                    Reached {
                        host: answered_as(&host, answered)?,
                        path: "direct",
                        through: endpoint,
                    },
                ))
            }),
        ));
    }
    for (relay, relay_node) in peers::relays(&registry, &[host]) {
        if relay_node.public_key == own {
            continue;
        }
        let (keypair, peer, request) = (keypair.clone(), peer.clone(), request.clone());
        let (relay, relay_node, host) = (relay.name.clone(), relay_node.clone(), host.to_string());
        attempts.push((
            format!("relay {relay}"),
            Box::pin(async move {
                let carried = through_relay(&relay, &relay_node, &host, &keypair).await?;
                let (stream, answered) = open(carried, &keypair, &peer, &request).await?;
                Ok((
                    stream,
                    Reached {
                        host: answered_as(&host, answered)?,
                        path: "relay",
                        through: relay,
                    },
                ))
            }),
        ));
    }
    if let (true, Request::Open { port }) = (through_this_node, &request) {
        match super::node::State::load() {
            // A connection to this host itself needs no node between.
            Ok(Some(state)) if state.host != host => {
                let (keypair, host, port) = (keypair.clone(), host.to_string(), *port);
                attempts.push((
                    format!("this host's node {}", state.host),
                    Box::pin(through_node(state, keypair, host, port)),
                ));
            }
            Ok(_) => {}
            Err(cause) => {
                attempts.push((
                    "this host's node".to_string(),
                    Box::pin(async move { Err(cause) }),
                ));
            }
        }
    }
    if attempts.is_empty() {
        return Err(format!(
            "registry host {host} declares no network endpoint and the registry declares no relay that could carry it; `stado network up` on {host} measures its endpoints, `stado network up --relay` on a reachable host declares a relay"
        ));
    }
    first(attempts).await.map_err(|failures| {
        format!(
            "no way to {} answered ({})",
            super::network_name(host),
            failures.join("; ")
        )
    })
}

/// The first attempt that succeeds, or every attempt's failure in the order
/// the attempts were made.
pub(crate) async fn first<T>(
    attempts: Vec<(String, BoxFuture<'static, Result<T, String>>)>,
) -> Result<T, Vec<String>> {
    let mut running = attempts
        .into_iter()
        .enumerate()
        .map(|(order, (what, attempt))| async move { (order, what, attempt.await) })
        .collect::<FuturesUnordered<_>>();
    let mut failures = Vec::new();
    while let Some((order, what, result)) = running.next().await {
        match result {
            Ok(value) => return Ok(value),
            Err(cause) => failures.push((order, format!("{what}: {cause}"))),
        }
    }
    failures.sort_by_key(|(order, _)| *order);
    Err(failures.into_iter().map(|(_, failure)| failure).collect())
}

/// A raw stream to `port` on registry host `host`'s loopback, opened by this
/// host's node (`state`) for this process over the loopback, which needs no
/// Local Network permission. The node checks the host it reaches answers as
/// `host`.
async fn through_node(
    state: super::node::State,
    keypair: Keypair,
    host: String,
    port: u16,
) -> Result<(DuplexStream, Reached), String> {
    let tcp = TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, state.port))
        .await
        .map_err(|error| format!("loopback port {}: {error}", state.port))?;
    let request = Request::Reach {
        host: host.clone(),
        port,
    };
    let (stream, _) = open(tcp, &keypair, &keypair.public, &request).await?;
    Ok((
        stream,
        Reached {
            host,
            path: "node",
            through: state.host,
        },
    ))
}

fn answered_as(expected: &str, answered: String) -> Result<String, String> {
    if answered == expected {
        Ok(answered)
    } else {
        Err(format!(
            "the node holding {expected}'s registry key answered as {answered}"
        ))
    }
}

/// A TCP connection to one declared `host:port` endpoint; a `.local` name
/// this machine's resolver does not answer is asked of the link.
pub(crate) async fn tcp(endpoint: &str) -> Result<TcpStream, String> {
    let (address, port) = endpoint
        .rsplit_once(':')
        .and_then(|(address, port)| {
            Some((address.trim_matches(['[', ']']), port.parse::<u16>().ok()?))
        })
        .ok_or_else(|| format!("endpoint {endpoint:?} is not host:port"))?;
    let addresses = crate::remote::lan::socket_addrs(address, port).await?;
    match TcpStream::connect(&addresses[..]).await {
        Ok(stream) => Ok(stream),
        // A refusal macOS gives a program it has not allowed the local network
        // reads like a missing route; it is named as the denial it is.
        Err(error) => Err(
            match crate::remote::lan::local_network(&error, &addresses).await {
                Some(denied) => format!("connect {endpoint}: {error}: {denied}"),
                None => format!("connect {endpoint}: {error}"),
            },
        ),
    }
}

/// Handshake with the node holding `peer`, ask `request`, and return the
/// stream with the host name the node answered as.
pub(crate) async fn open<S: Stream>(
    stream: S,
    keypair: &Keypair,
    peer: &[u8],
    request: &Request,
) -> Result<(DuplexStream, String), String> {
    let mut session = secure::initiate(stream, keypair, peer).await?;
    session.send(request).await?;
    match session.receive::<Answer>().await? {
        Answer::Ok { host } => Ok((session.into_plain(), host)),
        Answer::Refused { reason } => Err(format!("the node refused: {reason}")),
        Answer::Dial { .. } => {
            Err("the node answered a dial-back to a request that asked for none".to_string())
        }
    }
}

/// A raw stream to `host`'s node, carried by relay `relay`.
async fn through_relay(
    relay: &str,
    node: &NetworkNode,
    host: &str,
    keypair: &Keypair,
) -> Result<DuplexStream, String> {
    let key =
        key::decode_public(&node.public_key).map_err(|cause| format!("relay {relay}: {cause}"))?;
    let request = Request::Relay {
        host: host.to_string(),
    };
    let attempts = node
        .endpoints
        .iter()
        .map(|endpoint| {
            let what = format!("endpoint {endpoint}");
            let (endpoint, key, keypair, request) = (
                endpoint.clone(),
                key.clone(),
                keypair.clone(),
                request.clone(),
            );
            let attempt: BoxFuture<'static, Result<DuplexStream, String>> = Box::pin(async move {
                let tcp = tcp(&endpoint).await?;
                open(tcp, &keypair, &key, &request)
                    .await
                    .map(|(stream, _)| stream)
            });
            (what, attempt)
        })
        .collect::<Vec<_>>();
    if attempts.is_empty() {
        return Err("the relay declares no endpoint".to_string());
    }
    first(attempts)
        .await
        .map_err(|failures| failures.join("; "))
}
