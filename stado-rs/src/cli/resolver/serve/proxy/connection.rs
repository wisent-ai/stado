//! One connection: resolve where it should go, open it, and copy both ways.

use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

use crate::service_resolution::ResolverAdapter;
use crate::cli::resolver::authority::paths::resolved_ssh_paths;
use crate::cli::resolver::serve::state::ResolverState;

use super::idle::{copy_until_idle, Activity};
use super::refusal::{refuse_connection, http_request_head, UPSTREAM_REFUSAL};

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
    let idle = Duration::from_secs(adapter.idle_seconds);
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
    // Both halves of the fleet reach the upstream over a plain socket: the host
    // that holds the store dials it directly, every other host dials the
    // forward this resolver keeps to it. One code path, and neither branch
    // below starts a process.
    //
    // That last sentence was false for as long as it stood here, and it is
    // worth saying why rather than trusting it again. It read "no process per
    // request on either" while the `else` branch called
    // `select_resolver_ssh_path` right here, ahead of the pool: every host
    // that declares an `ssh_fallbacks` entry -- every host in this fleet --
    // paid one `ssh <destination> true`, bounded at twenty seconds, in front
    // of every accepted connection. The claim was about the forward and was
    // written as though it covered the whole function.
    //
    // So it is now checkable against what is visible below: this branch dials
    // and nothing else. Every process the transport needs -- the path probe
    // and the forward itself -- is started by
    // [`ResolverState::tunnel_connect`], under the pool lock, once per
    // forward. A request that finds a warm forward costs the two sockets it
    // would cost anyway.
    let (mut client_read, mut client_write) = client.into_split();
    let upstream = if resolved.active_host == state.local_target {
        TcpStream::connect((host, port))
            .await
            .map_err(|error| format!("local upstream connect failed: {error}"))
    } else {
        let paths = resolved_ssh_paths(&resolved);
        state
            .tunnel_connect(&resolved.active_host, &paths, host, port)
            .await
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
    let (mut upstream_read, mut upstream_write) = upstream.into_split();
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
