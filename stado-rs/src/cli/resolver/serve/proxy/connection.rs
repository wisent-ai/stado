//! One connection: resolve where it should go, open it, and copy both ways.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt};
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::TcpStream;

use crate::cli::resolver::authority::tunnel::Tunnel;

use crate::cli::resolver::authority::paths::resolved_ssh_paths;
use crate::cli::resolver::serve::state::ResolverState;
use crate::service_resolution::ResolverAdapter;

use super::refusal::refuse_connection;

enum Upstream {
    Local(TcpStream),
    Remote(russh::ChannelStream<russh::client::Msg>, Arc<Tunnel>),
}

/// Channel opens sent to a remote host that have not been answered yet.
static OPENS_IN_FLIGHT: AtomicUsize = AtomicUsize::new(0);

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
        // Every open that has not answered yet is counted and named, so a
        // connection the adapter holds without end leaves a line saying which
        // host it waits on and for how long. On 2026-09-30 directory connects
        // waited 2 to 11 minutes behind this adapter while the log held only
        // the opens that failed at once, so the one that never answered could
        // not be told apart.
        let waiting = OPENS_IN_FLIGHT.fetch_add(1, Ordering::SeqCst) + 1;
        let started = std::time::Instant::now();
        eprintln!(
            "stado resolver service={} consumer={} opening channel to {host}:{port} on {:?}; \
             {waiting} open(s) now waiting for an answer",
            adapter.service, adapter.consumer, resolved.active_host
        );
        let opened = state
            .tunnel_connect(&resolved.active_host, &paths, host, port)
            .await
            .map(|(stream, session)| Upstream::Remote(stream, session))
            .map_err(|error| format!("active host {:?}: {error}", resolved.active_host));
        let still = OPENS_IN_FLIGHT.fetch_sub(1, Ordering::SeqCst) - 1;
        eprintln!(
            "stado resolver service={} consumer={} channel to {host}:{port} answered {} after {} ms; \
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
    match upstream {
        Upstream::Local(stream) => relay(client_read, client_write, stream, host, port).await,
        Upstream::Remote(stream, session) => {
            let started = std::time::Instant::now();
            // `channel closed` alone cannot say whether the service on the
            // remote host dropped this one connection or the whole SSH
            // session to that host died under every channel at once; the
            // session's own state after the failure is that answer.
            let result = relay(client_read, client_write, stream, host, port)
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

/// Copy both directions until each side closes. A connection ends when its
/// client or its service ends it, and a failed copy is reported with the
/// transport's own error.
async fn relay<S: AsyncRead + AsyncWrite + Unpin>(
    mut client_read: OwnedReadHalf,
    mut client_write: OwnedWriteHalf,
    upstream: S,
    host: &str,
    port: u16,
) -> Result<(), String> {
    let (mut upstream_read, mut upstream_write) = tokio::io::split(upstream);
    let upload = async {
        let sent = tokio::io::copy(&mut client_read, &mut upstream_write).await?;
        upstream_write.shutdown().await?;
        Ok::<u64, std::io::Error>(sent)
    };
    let download = async {
        let received = tokio::io::copy(&mut upstream_read, &mut client_write).await?;
        client_write.shutdown().await?;
        Ok::<u64, std::io::Error>(received)
    };
    tokio::try_join!(upload, download)
        .map(|_| ())
        .map_err(|error| format!("proxy to {host}:{port} failed: {error}"))
}
