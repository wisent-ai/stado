//! One connection: resolve where it should go, open it, and copy both ways.
//!
//! Two waits of one connection are published (`report::waiting`): the
//! channel open, until the destination host answers it, and the answer,
//! from the client's first byte going up the channel until the service's
//! first byte comes back. The second is the one a held service shows: the
//! channel opens at once and the request on it is never answered.

use std::sync::Arc;

use tokio::net::TcpStream;

use crate::cli::resolver::authority::tunnel::Tunnel;
use crate::cli::resolver::report::waiting::{self, Phase};

use crate::cli::resolver::authority::paths::resolved_ssh_paths;
use crate::cli::resolver::serve::state::ResolverState;
use crate::service_resolution::ResolverAdapter;
use crate::wait;

use super::refusal::refuse_connection;
use super::relay::{relay, Hold};

enum Upstream {
    Local(TcpStream),
    Remote(russh::ChannelStream<russh::client::Msg>, Arc<Tunnel>),
}

pub(super) async fn proxy_connection(
    client: TcpStream,
    adapter: &ResolverAdapter,
    state: &ResolverState,
) -> Result<(), String> {
    // The client's own address is the one fact that ties a held connection
    // to the process holding it; a client whose address cannot be read is
    // recorded without one rather than refused.
    let client_address = match client.peer_addr() {
        Ok(address) => address.to_string(),
        Err(error) => {
            eprintln!(
                "stado resolver service={} consumer={} client address unreadable: {error}",
                adapter.service, adapter.consumer
            );
            String::new()
        }
    };
    let (client_read, mut client_write) = client.into_split();
    // A route this resolver cannot relay - unresolvable, or resolving to one
    // of this process's own listeners, which would loop the connection back
    // into this process with nothing to answer it - is refused to the client
    // at once with the cause, never closed silently and never relayed.
    let resolved = match state.resolve(&adapter.service, &adapter.consumer).await {
        Ok(resolved) => resolved,
        Err(cause) => {
            refuse_connection(&mut client_write, adapter, &adapter.bind, &cause).await;
            return Ok(());
        }
    };
    eprintln!(
        "stado resolver service={} consumer={} generation={} destination={}",
        adapter.service, adapter.consumer, resolved.generation, resolved.active_host
    );
    let endpoint = url::Url::parse(&resolved.endpoint.url)
        .map_err(|error| format!("invalid resolved endpoint: {error}"))?;
    let host = endpoint
        .host_str()
        .ok_or_else(|| "resolved endpoint has no host".to_string())?;
    let port = endpoint
        .port_or_known_default()
        .ok_or_else(|| "resolved endpoint has no port".to_string())?;
    // A connection on an adapter this resolver already holds unanswered
    // longer than its refresh interval would stand behind the held ones when
    // they stand on the session it would ride. That session is replaced by a
    // new one to the same host when one answers
    // ([`ResolverState::renew_standing_session`]); otherwise the connection
    // is refused at once with the held wait's own sentence and why no new
    // session carries it, so a client learns in a millisecond what it would
    // otherwise learn never, and the held channel keeps its place in
    // `stado resolver status`.
    let own_bind = url::Url::parse(&format!("http://{}", adapter.bind)).map_err(|error| {
        format!(
            "this adapter's bind {} is not an address: {error}",
            adapter.bind
        )
    })?;
    if let Some(held) = waiting::held_at(&own_bind)? {
        let endpoint = format!("{host}:{port}");
        let youngest =
            waiting::host::youngest_held_seconds(&adapter.bind, &resolved.active_host, &endpoint)?;
        let carried = match (resolved.active_host == state.local_target, youngest) {
            (false, Some(youngest)) => {
                state
                    .renew_standing_session(
                        &adapter.bind,
                        &resolved.active_host,
                        &resolved_ssh_paths(&resolved),
                        host,
                        port,
                        youngest,
                    )
                    .await
            }
            (true, _) => Err(format!(
                "{endpoint} is on this host, so no SSH session carries it to replace"
            )),
            (false, None) => Err(format!(
                "no wait to {endpoint} on {} stands, so the held ones ride no session to replace",
                resolved.active_host
            )),
        };
        match carried {
            Ok(renewed) => eprintln!(
                "stado resolver service={} consumer={} relaying although the adapter holds \
                 connections: {}; {held}",
                adapter.service,
                adapter.consumer,
                match renewed {
                    Some(renewed) => renewed,
                    None => format!(
                        "the held waits stand on a session to {} this resolver no longer \
                         sends on, so the connection rides the current one",
                        resolved.active_host
                    ),
                }
            ),
            Err(cause) => {
                refuse_connection(
                    &mut client_write,
                    adapter,
                    &endpoint,
                    &format!("not relayed: {held}; {cause}"),
                )
                .await;
                return Ok(());
            }
        }
    }
    // Remote traffic opens channels on the resolver's native SSH session.
    // There is no child process or intermediary TCP listener.
    let upstream = if resolved.active_host == state.local_target {
        wait::until(
            wait::Kind::Network,
            format!("a TCP connection for {}", adapter.service),
            format!("{host}:{port}"),
            TcpStream::connect((host, port)),
        )
        .await
        .map_err(|error| format!("local upstream connect failed: {error}"))
        .map(Upstream::Local)
    } else {
        let paths = resolved_ssh_paths(&resolved);
        // Every open that has not answered yet is recorded, published and
        // named, so a connection the adapter holds without end shows in
        // `stado resolver status` with the host it waits on and since when,
        // not only as a running count in this log.
        let endpoint = format!("{host}:{port}");
        let (key, waiting) = waiting::begin(
            &adapter.service,
            &adapter.consumer,
            &adapter.bind,
            &resolved.active_host,
            &endpoint,
            Phase::Open,
            &client_address,
        );
        let started = std::time::Instant::now();
        eprintln!(
            "stado resolver service={} consumer={} opening channel to {endpoint} on {:?}; \
             {waiting} open(s) now waiting for an answer",
            adapter.service, adapter.consumer, resolved.active_host
        );
        let opened = state
            .tunnel_connect(&resolved.active_host, &paths, host, port)
            .await
            .map(|(stream, session)| Upstream::Remote(stream, session))
            .map_err(|error| format!("active host {:?}: {error}", resolved.active_host));
        let still = waiting::end(key);
        eprintln!(
            "stado resolver service={} consumer={} channel to {endpoint} answered {} after {} ms; \
             {still} open(s) still waiting",
            adapter.service,
            adapter.consumer,
            if opened.is_ok() { "open" } else { "refused" },
            started.elapsed().as_millis()
        );
        opened
    };
    // A refusal is written to the client before anything is read from it, so
    // the sentence names the reason instead of a socket that closed silently.
    let upstream = match upstream {
        Ok(upstream) => upstream,
        Err(cause) => {
            refuse_connection(
                &mut client_write,
                adapter,
                &format!("{host}:{port}"),
                &cause,
            )
            .await;
            return Ok(());
        }
    };
    let hold = Hold::new(
        adapter.clone(),
        resolved.active_host.clone(),
        format!("{host}:{port}"),
        client_address,
    );
    match upstream {
        Upstream::Local(stream) => relay(client_read, client_write, stream, host, port, hold).await,
        Upstream::Remote(stream, session) => {
            let started = std::time::Instant::now();
            // `channel closed` alone cannot say whether the service on the
            // remote host dropped this one connection or the whole SSH
            // session to that host died under every channel at once; the
            // session's own state after the failure is that answer.
            let result = relay(client_read, client_write, stream, host, port, hold)
                .await
                .map_err(|error| {
                    let session_state = if session.usable() {
                        "still open, so the service end closed this one channel"
                    } else {
                        "closed, so every channel on it failed together"
                    };
                    format!(
                        "{error} after {} ms; the SSH session to {:?} is {session_state}",
                        started.elapsed().as_millis(),
                        resolved.active_host,
                    )
                });
            drop(session);
            result
        }
    }
}
