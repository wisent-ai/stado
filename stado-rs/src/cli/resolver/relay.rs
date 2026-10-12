//! `stado resolver relay SERVICE --consumer CONSUMER --bind ADDRESS`: one
//! caller's own route to a service the directory places on another host,
//! for as long as the caller holds this process.
//!
//! The host resolver binds every declared adapter and relays each connection
//! over its native SSH session to the service's active host. A reader that
//! must reach a service while that resolver holds no listener (it stopped,
//! or this host declares no adapter for the pair) had no route at all: the
//! address `service directory connect` hands out led to a socket nobody
//! opened, and every judge, review and desktop client behind it stood still
//! although the service answered on its own host and the fleet channel to
//! that host was open. This verb is that route, owned by one caller: the
//! directory is read the way every registry read is (from the authority),
//! the service is resolved for the named consumer exactly as the resolver
//! resolves it, the loopback address the caller names (one this host handed
//! out with `stado host local port`) is bound, its URL is printed
//! first, and each connection accepted there is carried over the same SSH
//! channel the resolver would open. The listener lives until the caller
//! closes this process's standard input or ends it; a relay nobody owns does
//! not outlive its owner.
//!
//! A service placed on this host needs no relay: its loopback address is
//! printed and the process ends, so a caller that always asks here gets one
//! answer shape on every host.

use std::net::SocketAddr;
use std::sync::Arc;

use serde_json::json;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};

use crate::cli::resolver::authority::paths::resolved_ssh_paths;
use crate::cli::resolver::authority::tunnel::Tunnel;
use crate::cli::CmdError;
use crate::service_resolution::ResolvedService;
use crate::wait;

/// The relay's one answer line, before it serves: where the caller dials.
fn announce(resolved: &ResolvedService, url: &str, relayed: bool, json_output: bool) {
    if json_output {
        println!(
            "{}",
            json!({
                "service": resolved.name,
                "placed_on": resolved.active_host,
                "generation": resolved.generation,
                "url": url,
                "relayed": relayed,
            })
        );
    } else if relayed {
        println!(
            "{url}  ({} on {}, relayed over this process's SSH channel until it ends)",
            resolved.name, resolved.active_host
        );
    } else {
        println!("{url}  ({} on this host, no relay needed)", resolved.name);
    }
}

/// The host and port the resolved endpoint names on its own host.
fn endpoint_of(resolved: &ResolvedService) -> Result<(String, u16), CmdError> {
    let endpoint = url::Url::parse(&resolved.endpoint.url).map_err(|error| {
        CmdError::declaration(format!(
            "{} on {} declares {}, which is not a URL: {error}",
            resolved.name, resolved.active_host, resolved.endpoint.url
        ))
    })?;
    let host = endpoint.host_str().ok_or_else(|| {
        CmdError::declaration(format!(
            "{} on {} declares {}, which names no host",
            resolved.name, resolved.active_host, resolved.endpoint.url
        ))
    })?;
    let port = endpoint.port_or_known_default().ok_or_else(|| {
        CmdError::declaration(format!(
            "{} on {} declares {}, which names no port",
            resolved.name, resolved.active_host, resolved.endpoint.url
        ))
    })?;
    Ok((host.to_string(), port))
}

/// The caller's loopback address, parsed and bound.
async fn bind(address: &str) -> Result<(TcpListener, SocketAddr), CmdError> {
    let parsed: SocketAddr = address.parse().map_err(|error| {
        CmdError::usage(format!(
            "--bind {address:?} is not a host:port address: {error}"
        ))
    })?;
    if !parsed.ip().is_loopback() {
        return Err(CmdError::refused(format!(
            "--bind {address} is not a loopback address; a relay is for this host's own callers"
        )));
    }
    let listener = TcpListener::bind(parsed).await.map_err(|error| {
        CmdError::click(format!(
            "could not bind {address} for the relay: {error}; take an address this host hands out \
             now with `stado host local port`"
        ))
        .stating(crate::cli::entry::error::io_failure_code(error.kind()))
    })?;
    let bound = listener
        .local_addr()
        .map_err(|error| CmdError::click(format!("the relay listener has no address: {error}")))?;
    Ok((listener, bound))
}

pub(super) async fn relay(
    service: &str,
    consumer: &str,
    address: &str,
    json_output: bool,
) -> Result<(), CmdError> {
    let (resolved, here) = super::resolved_for(service, consumer).await?;
    let (host, port) = endpoint_of(&resolved)?;
    if resolved.active_host == here {
        announce(&resolved, &resolved.endpoint.url, false, json_output);
        return Ok(());
    }
    let paths = resolved_ssh_paths(&resolved);
    if paths.is_empty() {
        return Err(CmdError::declaration(format!(
            "{} is placed on {}, whose registry record carries no SSH connection path to relay over",
            resolved.name, resolved.active_host
        )));
    }
    // The session is opened before the listener is announced, so a host the
    // fleet channel cannot reach is the refusal the caller gets, not an
    // address that accepts and then fails.
    let tunnel = wait::until(
        wait::Kind::Host,
        format!("an SSH session for {}", resolved.name),
        resolved.active_host.clone(),
        async { Tunnel::open(&paths).await.map_err(std::io::Error::other) },
    )
    .await
    .map_err(|error| {
        CmdError::click(format!(
            "{} is placed on {}, which the fleet channel could not reach: {error}",
            resolved.name, resolved.active_host
        ))
        .stating(crate::primitives::failure::FailureCode::InfraDown)
    })?;
    let (listener, bound) = bind(address).await?;
    announce(&resolved, &format!("http://{bound}"), true, json_output);
    serve(listener, Arc::new(tunnel), &resolved, &host, port).await
}

/// Accept connections until the owner lets go of standard input.
async fn serve(
    listener: TcpListener,
    tunnel: Arc<Tunnel>,
    resolved: &ResolvedService,
    host: &str,
    port: u16,
) -> Result<(), CmdError> {
    let mut owner = BufReader::new(tokio::io::stdin());
    loop {
        let mut line = String::new();
        tokio::select! {
            accepted = listener.accept() => {
                let (client, peer) = accepted.map_err(|error| {
                    CmdError::click(format!("the relay listener stopped accepting: {error}"))
                })?;
                let tunnel = Arc::clone(&tunnel);
                let name = resolved.name.clone();
                let active_host = resolved.active_host.clone();
                let host = host.to_string();
                tokio::spawn(async move {
                    if let Err(cause) = carry(client, &tunnel, &host, port).await {
                        eprintln!(
                            "stado resolver relay {name}: connection from {peer} to {host}:{port} \
                             on {active_host} ended: {cause}"
                        );
                    }
                });
            }
            read = owner.read_line(&mut line) => {
                match read {
                    // Nothing read at all: the owner closed its end, and the
                    // relay has no owner left.
                    Ok(_) if line.is_empty() => return Ok(()),
                    Ok(_) => continue,
                    Err(error) => {
                        return Err(CmdError::click(format!(
                            "the relay's standard input could not be read, so its owner cannot be \
                             watched: {error}"
                        )));
                    }
                }
            }
        }
    }
}

/// One accepted connection, opened on the service's host and copied both
/// ways until either side ends it. A channel the host refuses is written to
/// the client as the refusal, so the client reads the cause, not a close.
async fn carry(
    mut client: TcpStream,
    tunnel: &Tunnel,
    host: &str,
    port: u16,
) -> Result<(), String> {
    let mut upstream = match tunnel.connect(host, port).await {
        Ok(stream) => stream,
        Err(cause) => {
            let refusal = format!("stado resolver relay: not relayed to {host}:{port}: {cause}\n");
            let _ = client.write_all(refusal.as_bytes()).await;
            return Err(cause);
        }
    };
    tokio::io::copy_bidirectional(&mut client, &mut upstream)
        .await
        .map(|_| ())
        .map_err(|error| format!("copy failed: {error}"))
}
