//! One connection: resolve where it should go, open it, and copy both ways.

use std::time::Duration;
use std::sync::Arc;

use tokio::net::TcpStream;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};

use crate::cli::resolver::authority::tunnel::Tunnel;

use crate::cli::resolver::authority::paths::resolved_ssh_paths;
use crate::cli::resolver::serve::state::ResolverState;
use crate::service_resolution::ResolverAdapter;

use super::idle::{copy_until_idle, Activity};
use super::refusal::refuse_connection;

enum Upstream {
    Local(TcpStream),
    Remote(russh::ChannelStream<russh::client::Msg>, Arc<Tunnel>),
}

pub(super) async fn proxy_connection(
    client: TcpStream,
    adapter: &ResolverAdapter,
    state: &ResolverState,
) -> Result<(), String> {
    let resolved = state.resolve(&adapter.service, &adapter.consumer).await?;
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
    // The directory is supposed to name where a service listens. When it names
    // this adapter's own bind instead, the proxy dials itself: the connection is
    // accepted, forwarded to the same socket, accepted again, and the reader
    // waits on a chain that never reaches a server. It looks exactly like a
    // healthy port with a hung backend, so say it instead of recursing.
    if format!("{host}:{port}") == adapter.bind {
        return Err(format!(
            "service {} resolves to {}, which is this adapter's own bind: the service \
             directory must carry the address the service listens on, not the stable \
             port published for its clients",
            adapter.service, adapter.bind
        ));
    }
    // Remote traffic opens channels on the resolver's native SSH session.
    // There is no child process or intermediary TCP listener.
    let (mut client_read, mut client_write) = client.into_split();
    let upstream = if resolved.active_host == state.local_target {
        TcpStream::connect((host, port))
            .await
            .map_err(|error| format!("local upstream connect failed: {error}"))
            .map(Upstream::Local)
    } else {
        let paths = resolved_ssh_paths(&resolved);
        state
            .tunnel_connect(&resolved.active_host, &paths, host, port)
            .await
            .map(|(stream, session)| Upstream::Remote(stream, session))
            .map_err(|error| format!("active host {:?}: {error}", resolved.active_host))
    };
    // A refusal is written to the client before anything is read from it, so
    // the sentence names the reason instead of a socket that closed silently.
    let upstream = match upstream {
        Ok(upstream) => upstream,
        Err(cause) => {
            refuse_connection(
                &mut client_read,
                &mut client_write,
                adapter,
                &format!("{host}:{port}"),
                &cause,
                None,
            )
            .await;
            return Ok(());
        }
    };
    match upstream {
        Upstream::Local(stream) => relay(client_read, client_write, stream, adapter, host, port).await,
        Upstream::Remote(stream, session) => {
            let result = relay(client_read, client_write, stream, adapter, host, port).await;
            drop(session);
            result
        }
    }
}

async fn relay<S: AsyncRead + AsyncWrite + Unpin>(
    mut client_read: OwnedReadHalf,
    mut client_write: OwnedWriteHalf,
    upstream: S,
    adapter: &ResolverAdapter,
    host: &str,
    port: u16,
) -> Result<(), String> {
    let idle = Duration::from_secs(adapter.idle_seconds);
    let (mut upstream_read, mut upstream_write) = tokio::io::split(upstream);
    let activity = Activity::new();
    // What a client reads instead of a silent close: the service that did not
    // answer, and the window it was measured against. `Connection: close` and
    // an exact length, so a client library parses it as a complete message.
    let body = format!(
        "service {} did not answer within the {}s idle window this adapter declares\n",
        adapter.service,
        idle.as_secs()
    );
    let timeout_answer = format!(
        "HTTP/1.1 504 Gateway Timeout\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let upload = copy_until_idle(&mut client_read, &mut upstream_write, idle, &activity, None);
    let download = copy_until_idle(
        &mut upstream_read,
        &mut client_write,
        idle,
        &activity,
        Some(timeout_answer.as_bytes()),
    );
    let (sent, received) =
        tokio::try_join!(upload, download).map_err(|error| format!("proxy failed: {error}"))?;
    // A cut connection is the proxy's decision, and a client that received a
    // truncated answer has to be able to read whose decision it was and after
    // how long. Silence here is what made `connection closed before message
    // completed` unattributable for four release runs.
    if sent.cut || received.cut {
        eprintln!(
            "stado resolver service={} consumer={} endpoint={host}:{port} closed an idle \
             connection after {}s with nothing moving in either direction: {} byte(s) to the \
             service, {} byte(s) back",
            adapter.service,
            adapter.consumer,
            idle.as_secs(),
            sent.bytes,
            received.bytes,
        );
    }
    Ok(())
}
