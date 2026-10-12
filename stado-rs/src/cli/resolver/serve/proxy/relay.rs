//! The relay of one opened connection: both directions copied until each
//! side closes, the request and the answer noticed on their first byte and
//! published as the connection's wait (`report::waiting`).

use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
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
    /// Whether the service's first byte has come down this connection.
    answer_began: AtomicBool,
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
            answer_began: AtomicBool::new(false),
        })
    }

    /// The client's first byte went up: the service now owes an answer.
    fn requested(&self) {
        let mut key = self.key();
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
        let mut key = self.key();
        let Some(taken) = key.take() else { return };
        let still = waiting::end(taken);
        eprintln!(
            "stado resolver service={} consumer={} request to {} on {:?} {how}; {still} open(s) \
             still waiting",
            self.adapter.service, self.adapter.consumer, self.endpoint, self.active_host
        );
    }

    /// The taken place, if any. It is set or taken whole, so a panic while it
    /// was held leaves it consistent and the lock is recovered: left
    /// poisoned, a taken place would never be given back and the resolver
    /// would count an answer as owed for as long as it runs.
    fn key(&self) -> std::sync::MutexGuard<'_, Option<u64>> {
        match self.key.lock() {
            Ok(key) => key,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    /// The service's first byte came down.
    fn answer_began(&self) {
        self.answer_began.store(true, Ordering::Release);
        self.answered("answered");
    }

    /// The client closed before the service's first byte: the request, if
    /// one went up, has no caller left, and the place is given back.
    fn abandoned(&self) {
        self.answered("abandoned by its client before an answer; the channel is closed");
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
/// request: one the service never answers stays in the published list for
/// as long as its client waits for it. A client that closes before the
/// answer began has nobody left to answer: the relay ends there, the
/// channel to the service is closed with it and the place is given back,
/// so a caller that died with its request sent does not hold the adapter
/// against every later caller.
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
    tokio::pin!(upload);
    tokio::pin!(download);
    let failed = |error: std::io::Error| format!("proxy to {host}:{port} failed: {error}");
    let client_ended = tokio::select! {
        sent = &mut upload => {
            sent.map_err(failed)?;
            true
        }
        received = &mut download => {
            received.map_err(failed)?;
            false
        }
    };
    // The client is gone before the service's first byte: whether it sent a
    // request or nothing at all, no answer has anybody to go to.
    if client_ended && !hold.answer_began.load(Ordering::Acquire) {
        hold.abandoned();
        return Ok(());
    }
    match client_ended {
        true => download.await.map(|_| ()).map_err(failed),
        false => upload.await.map(|_| ()).map_err(failed),
    }
}
