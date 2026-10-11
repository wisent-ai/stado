//! The relay of one opened connection: both directions copied until each
//! side closes, the request and the answer noticed on their first byte and
//! published as the connection's wait (`report::waiting`).

use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};

use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt, ReadBuf};
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};

use crate::cli::resolver::report::waiting::{self, Phase};
use crate::service_resolution::ResolverAdapter;

/// One relayed connection's place in the published waiting list: taken
/// when the client's first byte goes up the channel, given back when the
/// service's first byte comes down, or when the connection ends before one
/// does. The two halves of the relay share it; the last half dropped gives
/// the place back. No clock cuts a request: a service that never answers
/// stays in the list, where `stado resolver status` names it and every
/// caller that reads the list before sending is refused with its sentence.
pub(super) struct Hold {
    adapter: ResolverAdapter,
    active_host: String,
    endpoint: String,
    key: Mutex<Option<u64>>,
}

impl Hold {
    /// The place of one connection of `adapter` to `endpoint` on
    /// `active_host`, nothing sent yet.
    pub(super) fn new(
        adapter: ResolverAdapter,
        active_host: String,
        endpoint: String,
    ) -> Arc<Self> {
        Arc::new(Self {
            adapter,
            active_host,
            endpoint,
            key: Mutex::new(None),
        })
    }

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
/// are noticed on their first byte and recorded in `hold`. No clock cuts a
/// request: one the service never answers stays in the published list.
pub(super) async fn relay<S: AsyncRead + AsyncWrite + Unpin>(
    client_read: OwnedReadHalf,
    mut client_write: OwnedWriteHalf,
    upstream: S,
    host: &str,
    port: u16,
    hold: Arc<Hold>,
) -> Result<(), String> {
    let (upstream_read, mut upstream_write) = tokio::io::split(upstream);
    let mut client_read = Noticed::new(client_read, Arc::clone(&hold), Hold::requested);
    let mut upstream_read = Noticed::new(upstream_read, Arc::clone(&hold), Hold::answer_began);
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
