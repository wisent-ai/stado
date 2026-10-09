//! One connection: resolve where it should go, open it, and copy both ways.
//!
//! Two waits of one connection are published (`report::waiting`): the
//! channel open, until the destination host answers it, and the answer,
//! from the client's first byte going up the channel until the service's
//! first byte comes back. The second is the one a held service shows: the
//! channel opens at once and the request on it is never answered.

use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};

use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt, ReadBuf};
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::TcpStream;

use crate::cli::resolver::authority::tunnel::Tunnel;
use crate::cli::resolver::report::waiting::{self, Phase};

use crate::cli::resolver::authority::paths::resolved_ssh_paths;
use crate::cli::resolver::serve::state::ResolverState;
use crate::service_resolution::ResolverAdapter;
use crate::wait;

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
    let (client_read, mut client_write) = client.into_split();
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
    let hold = Arc::new(Hold {
        adapter: adapter.clone(),
        active_host: resolved.active_host.clone(),
        endpoint: format!("{host}:{port}"),
        key: Mutex::new(None),
    });
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

/// One relayed connection's place in the published waiting list: taken
/// when the client's first byte goes up the channel, given back when the
/// service's first byte comes down, or when the connection ends before one
/// does. The two halves of the relay share it; the last half dropped gives
/// the place back.
struct Hold {
    adapter: ResolverAdapter,
    active_host: String,
    endpoint: String,
    key: Mutex<Option<u64>>,
}

impl Hold {
    /// The client's first byte went up: the service now owes an answer.
    fn requested(&self) {
        let Ok(mut key) = self.key.lock() else { return };
        if key.is_some() {
            return;
        }
        let (taken, waiting) = waiting::begin(
            &self.adapter.service,
            &self.adapter.consumer,
            &self.adapter.bind,
            &self.active_host,
            &self.endpoint,
            Phase::Answer,
        );
        *key = Some(taken);
        eprintln!(
            "stado resolver service={} consumer={} request sent to {} on {:?}; {waiting} open(s) \
             now waiting for an answer",
            self.adapter.service, self.adapter.consumer, self.endpoint, self.active_host
        );
    }

    /// The service's first byte came down, or the connection ended without
    /// one: the place is given back.
    fn answered(&self, how: &str) {
        let Ok(mut key) = self.key.lock() else { return };
        let Some(taken) = key.take() else { return };
        let still = waiting::end(taken);
        eprintln!(
            "stado resolver service={} consumer={} request to {} on {:?} {how}; {still} open(s) \
             still waiting",
            self.adapter.service, self.adapter.consumer, self.endpoint, self.active_host
        );
    }

    /// The service's first byte came down.
    fn answer_began(&self) {
        self.answered("answered");
    }
}

impl Drop for Hold {
    fn drop(&mut self) {
        self.answered("ended before an answer");
    }
}

/// One half of a relayed connection, read through, that tells the shared
/// [`Hold`] when its first byte arrives: the client's half that a request
/// was sent, the service's half that an answer began.
struct Noticed<R> {
    inner: R,
    hold: Arc<Hold>,
    noticed: bool,
    first: fn(&Hold),
}

impl<R> Noticed<R> {
    fn new(inner: R, hold: Arc<Hold>, first: fn(&Hold)) -> Self {
        Self {
            inner,
            hold,
            noticed: false,
            first,
        }
    }
}

impl<R: AsyncRead + Unpin> AsyncRead for Noticed<R> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        let before = buf.filled().len();
        let polled = Pin::new(&mut self.inner).poll_read(cx, buf);
        if !self.noticed && matches!(polled, Poll::Ready(Ok(()))) && buf.filled().len() > before {
            self.noticed = true;
            (self.first)(&self.hold);
        }
        polled
    }
}

/// Copy both directions until each side closes. A connection ends when its
/// client or its service ends it, and a failed copy is reported with the
/// transport's own error. The request going up and the answer coming down
/// are noticed on their first byte and recorded in `hold`.
async fn relay<S: AsyncRead + AsyncWrite + Unpin>(
    client_read: OwnedReadHalf,
    mut client_write: OwnedWriteHalf,
    upstream: S,
    host: &str,
    port: u16,
    hold: Arc<Hold>,
) -> Result<(), String> {
    let (upstream_read, mut upstream_write) = tokio::io::split(upstream);
    let mut client_read = Noticed::new(client_read, Arc::clone(&hold), Hold::requested);
    let mut upstream_read = Noticed::new(upstream_read, hold, Hold::answer_began);
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
