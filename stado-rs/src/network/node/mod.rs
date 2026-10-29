//! `stado network serve`: this host's network node. It listens on the port
//! `stado network up` recorded, admits only peers whose key the registry
//! declares for a fleet host, and opens for them a port this host's own
//! registry SSH paths name. A relay node also carries connections to other
//! hosts; every other node keeps a standing attachment to each relay its
//! registry copy declares, so a host no peer can dial is still reached.

use std::net::Ipv4Addr;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tokio::net::{TcpListener, TcpStream};

use super::key::{self, Keypair};
use super::secure::{self, Stream};
use super::{peers, Answer, Request};

mod attach;
pub mod declaration;
mod hub;
pub mod serving;

pub(crate) use hub::Hub;

/// What `stado network up` recorded for this account's node.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct State {
    /// The registry host this node is.
    pub host: String,
    /// The port the node listens on, as the system handed it out.
    pub port: u16,
}

impl State {
    pub fn path() -> Result<std::path::PathBuf, String> {
        Ok(key::directory()?.join("node.json"))
    }

    /// The recorded state, `None` when `up` never ran for this account.
    pub fn load() -> Result<Option<Self>, String> {
        let path = Self::path()?;
        match std::fs::read_to_string(&path) {
            Ok(text) => serde_json::from_str(&text).map(Some).map_err(|error| {
                format!(
                    "{} is not a Stado network node state: {error}",
                    path.display()
                )
            }),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(format!("read {}: {error}", path.display())),
        }
    }

    pub fn store(&self) -> Result<(), String> {
        let path = Self::path()?;
        let text = serde_json::to_string_pretty(self)
            .map_err(|error| format!("encode node state: {error}"))?;
        crate::targets::write_atomic(&path, &text)
            .map_err(|error| format!("write {}: {error}", path.display()))
    }

    pub fn remove() -> Result<(), String> {
        let path = Self::path()?;
        match std::fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(format!("remove {}: {error}", path.display())),
        }
    }
}

/// What every connection of this node shares.
pub(crate) struct Context {
    keypair: Keypair,
    host: String,
    hub: Option<Hub>,
}

/// Run this account's node until the process is ended, publishing that it
/// listens, or why it could not, for `stado network up` to read.
pub async fn serve() -> Result<(), String> {
    let (listener, context, port) = match listen().await {
        Ok(listening) => listening,
        Err(reason) => {
            let failed = serving::Serving {
                pid: std::process::id(),
                port: 0,
                state: serving::FAILED.to_string(),
                reason: Some(reason.clone()),
            };
            if let Err(cause) = serving::publish(&failed) {
                eprintln!("stado network: {cause}");
            }
            return Err(reason);
        }
    };
    serving::publish(&serving::Serving {
        pid: std::process::id(),
        port,
        state: serving::SERVING.to_string(),
        reason: None,
    })?;
    eprintln!("stado network: {} listening on port {port}", context.host);
    accept_forever(listener, context).await
}

async fn listen() -> Result<(TcpListener, Arc<Context>, u16), String> {
    let keypair = key::load()?;
    let state = State::load()?.ok_or_else(|| {
        format!(
            "this account has no Stado network node state at {}; `stado network up` records it",
            State::path().map_or_else(|cause| cause, |path| path.display().to_string())
        )
    })?;
    let (_, node) = peers::find(&state.host).await?;
    if node.public_key != keypair.public_base64() {
        return Err(format!(
            "registry host {} declares network key {} but this account holds {}; `stado network up` on this host declares the key it holds",
            state.host,
            node.public_key,
            keypair.public_base64()
        ));
    }
    let listener = TcpListener::bind((Ipv4Addr::UNSPECIFIED, state.port))
        .await
        .map_err(|error| {
            format!(
                "the node cannot listen on port {} that `stado network up` recorded: {error}; `stado network down` then `stado network up` takes a port the system hands out and declares it",
                state.port
            )
        })?;
    let context = Arc::new(Context {
        keypair,
        host: state.host.clone(),
        hub: node.relay.map(|_| Hub::default()),
    });
    if context.hub.is_none() {
        let registry = peers::copy().await?;
        for (relay, _) in peers::relays(&registry, &[state.host.as_str()]) {
            tokio::spawn(attach::keep(Arc::clone(&context), relay.name.clone()));
        }
    }
    Ok((listener, context, state.port))
}

async fn accept_forever(listener: TcpListener, context: Arc<Context>) -> Result<(), String> {
    loop {
        match listener.accept().await {
            Ok((stream, from)) => {
                let context = Arc::clone(&context);
                tokio::spawn(async move {
                    if let Err(cause) = handle(stream, context).await {
                        eprintln!("stado network: connection from {from}: {cause}");
                    }
                });
            }
            Err(error) => eprintln!("stado network: accepting a connection failed: {error}"),
        }
    }
}

/// The fleet host whose registry key `remote` is, asking the authority once
/// when this host's copy does not know it.
async fn admitted(remote: &[u8]) -> Result<String, String> {
    if let Some(host) = peers::host_with_key(&peers::copy().await?, remote) {
        return Ok(host);
    }
    if let Some(host) = peers::refreshed()
        .await
        .and_then(|fresh| peers::host_with_key(&fresh, remote))
    {
        return Ok(host);
    }
    Err(format!(
        "no registry host declares network key {}; `stado network up` on the connecting host declares it",
        base64::Engine::encode(&base64::engine::general_purpose::STANDARD, remote)
    ))
}

/// One connection: the handshake, the peer's admission, its request.
pub(crate) async fn handle<S: Stream>(stream: S, context: Arc<Context>) -> Result<(), String> {
    let pending = secure::accept(stream, &context.keypair).await?;
    let admitted = admitted(&pending.remote).await;
    let mut session = pending.complete().await?;
    let peer = match admitted {
        Ok(peer) => peer,
        Err(reason) => {
            session
                .send(&Answer::Refused {
                    reason: reason.clone(),
                })
                .await?;
            return Err(reason);
        }
    };
    let request: Request = session.receive().await?;
    let hub = match (&request, &context.hub) {
        (Request::Probe | Request::Open { .. }, _) => None,
        (_, Some(hub)) => Some(hub),
        (_, None) => {
            let reason = format!(
                "{} is no relay; its registry node declares no relay",
                context.host
            );
            session
                .send(&Answer::Refused {
                    reason: reason.clone(),
                })
                .await?;
            return Err(format!("{peer}: {reason}"));
        }
    };
    match (request, hub) {
        (Request::Probe, _) => {
            session
                .send(&Answer::Ok {
                    host: context.host.clone(),
                })
                .await
        }
        (Request::Open { port }, _) => {
            let target = match open_port(&context.host, port).await {
                Ok(target) => target,
                Err(reason) => {
                    session
                        .send(&Answer::Refused {
                            reason: reason.clone(),
                        })
                        .await?;
                    return Err(format!("{peer}: {reason}"));
                }
            };
            session
                .send(&Answer::Ok {
                    host: context.host.clone(),
                })
                .await?;
            splice(session.into_plain(), target).await
        }
        (Request::Relay { host }, Some(hub)) => {
            hub.carry(session, &host, &peer, &context.host).await
        }
        (Request::Attach, Some(hub)) => hub.attach(session, &peer, &context.host).await,
        (Request::Accept { token }, Some(hub)) => hub.accept(session, &token, &context.host).await,
        (_, None) => Ok(()),
    }
}

/// A connection to `port` on this host's loopback, when it is a port this
/// host's registry SSH paths name.
async fn open_port(host: &str, port: u16) -> Result<TcpStream, String> {
    let registry = peers::copy().await?;
    let target = registry
        .lookup(host)
        .ok_or_else(|| format!("this host's registry copy no longer declares {host}"))?;
    let ports = peers::ssh_ports(target);
    if !ports.contains(&port) {
        return Err(format!(
            "port {port} is not an SSH port registry host {host} declares (its paths name {ports:?}); the node opens only those"
        ));
    }
    TcpStream::connect((Ipv4Addr::LOCALHOST, port))
        .await
        .map_err(|error| format!("{host} opens no port {port} on its loopback: {error}"))
}

/// Copy both ways until either side ends.
pub(crate) async fn splice<A: Stream, B: Stream>(mut one: A, mut other: B) -> Result<(), String> {
    tokio::io::copy_bidirectional(&mut one, &mut other)
        .await
        .map(|_| ())
        .map_err(|error| format!("the carried connection ended: {error}"))
}
