//! The relay of one opened connection: both directions copied until each
//! side closes, the request and the answer noticed on their first byte and
//! published as the connection's wait (`report::waiting`).

use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};

use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt, ReadBuf};
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::sync::watch;

use crate::cli::resolver::report::waiting::{self, Phase};
use crate::service_resolution::ResolverAdapter;

use super::refusal::refuse_connection;
use super::shed;

/// One relayed connection's place in the published waiting list, made at
/// the accept: taken when the client's first byte goes up the channel, given
/// back when the service's first byte comes down or the connection ends
/// first. The relay's two halves share it; the last dropped gives it back.
/// No clock cuts a request: a service that never answers stays in the list,
/// where `stado resolver status` names it and every caller that reads the
/// list before sending is refused with its sentence.
pub(super) struct Hold {
    adapter: ResolverAdapter,
    active_host: String,
    endpoint: String,
    /// The loopback address the client connected from.
    client: String,
    /// When the client was accepted: its wait on an answer counts from here.
    accepted: String,
    key: Mutex<Option<u64>>,
    /// Whether the service's first byte has come down this connection.
    answer_began: AtomicBool,
    /// How this side ends the wait: the relay listens on it, and the shed
    /// sends the reason when the listener runs out of descriptors.
    ender: shed::Ender,
}

impl Hold {
    /// The place of one accepted connection of `adapter` to `endpoint` on `active_host`.
    pub(super) fn new(
        adapter: ResolverAdapter,
        active_host: String,
        endpoint: String,
        client: String,
    ) -> Arc<Self> {
        let (ender, _listening) = watch::channel(None);
        Arc::new(Self {
            adapter,
            active_host,
            endpoint,
            client,
            accepted: crate::cli::resolver::report::published::now_iso(),
            key: Mutex::new(None),
            answer_began: AtomicBool::new(false),
            ender,
        })
    }

    /// The relay's end of the ender.
    pub(super) fn shed_signal(&self) -> watch::Receiver<Option<String>> {
        self.ender.subscribe()
    }

    /// The client's first byte went up: the service now owes an answer. The
    /// first bytes are read for their request line, so the wait names what
    /// was asked.
    fn requested(&self, first: &[u8]) {
        let mut key = self.key();
        if key.is_some() {
            return;
        }
        let request = request_line(first);
        let (taken, waiting) = waiting::begin(waiting::Began {
            service: &self.adapter.service,
            consumer: &self.adapter.consumer,
            bind: &self.adapter.bind,
            active_host: &self.active_host,
            endpoint: &self.endpoint,
            phase: Phase::Answer,
            client: &self.client,
            request: request.clone(),
        });
        waiting::since_accepted(taken, &self.accepted);
        *key = Some(taken);
        shed::listen(taken, self.ender.clone());
        let asked = match request {
            Some(line) => format!(" (`{line}`)"),
            None => String::new(),
        };
        eprintln!(
            "stado resolver service={} consumer={} request{asked} from {} sent to {} on {:?}; \
             {waiting} open(s) now waiting for an answer",
            self.adapter.service,
            self.adapter.consumer,
            self.client,
            self.endpoint,
            self.active_host
        );
    }

    /// The wait ended, answered or not: the place is given back.
    fn answered(&self, how: &str) {
        let mut key = self.key();
        let Some(taken) = key.take() else { return };
        shed::forget(taken);
        let answered = self.answer_began.load(Ordering::Acquire);
        let still =
            crate::cli::resolver::report::answers::ended(&self.adapter.service, answered, taken);
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
    fn answer_began(&self, _first: &[u8]) {
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

/// The request line in the first bytes a client sent, up to its first line
/// end, a byte that is not text shown as U+FFFD; a chunk without a line end,
/// or a line holding a control character, names nothing rather than a guess.
fn request_line(first: &[u8]) -> Option<String> {
    let end = first.iter().position(|byte| *byte == b'\n')?;
    let text = String::from_utf8_lossy(&first[..end]);
    let line = text.trim_end_matches('\r');
    let printable = line.chars().all(|character| !character.is_control());
    (printable && !line.is_empty()).then(|| line.to_owned())
}

/// One half of a relayed connection, read through, that tells the shared
/// [`Hold`] when its first bytes arrive and hands it those bytes: the
/// client's half that a request was sent, the service's that an answer began.
struct Noticed<R> {
    inner: R,
    hold: Arc<Hold>,
    noticed: bool,
    first: fn(&Hold, &[u8]),
}

impl<R> Noticed<R> {
    fn new(inner: R, hold: Arc<Hold>, first: fn(&Hold, &[u8])) -> Self {
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
            (self.first)(&self.hold, &buf.filled()[before..]);
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
    let mut shed_signal = hold.shed_signal();
    let copied = copy_both_ways(
        client_read,
        &mut client_write,
        upstream,
        host,
        port,
        &hold,
        &mut shed_signal,
    )
    .await?;
    // The shed ended this wait: the client is refused with the resolver's
    // own sentence, the place is given back and the descriptor freed.
    if let Copied::Shed(reason) = copied {
        refuse_connection(&mut client_write, &hold.adapter, &hold.endpoint, &reason).await;
        hold.answered(&format!("ended by the resolver: {reason}"));
    }
    Ok(())
}

/// How the two copies ended.
enum Copied {
    /// Both directions ran to their ends, or the client left first.
    Done,
    /// The shed told this relay to end, with the sentence the client gets.
    Shed(String),
}

async fn copy_both_ways<S: AsyncRead + AsyncWrite + Unpin>(
    client_read: OwnedReadHalf,
    client_write: &mut OwnedWriteHalf,
    upstream: S,
    host: &str,
    port: u16,
    hold: &Arc<Hold>,
    shed_signal: &mut watch::Receiver<Option<String>>,
) -> Result<Copied, String> {
    let (upstream_read, mut upstream_write) = tokio::io::split(upstream);
    let mut client_read = Noticed::new(client_read, Arc::clone(hold), Hold::requested);
    let mut upstream_read = Noticed::new(upstream_read, Arc::clone(hold), Hold::answer_began);
    let upload = async {
        let sent = tokio::io::copy(&mut client_read, &mut upstream_write).await?;
        upstream_write.shutdown().await?;
        Ok::<u64, std::io::Error>(sent)
    };
    let download = async {
        let received = tokio::io::copy(&mut upstream_read, client_write).await?;
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
        changed = shed_signal.changed() => {
            changed.map_err(|_| {
                "the resolver's ender for this relay closed while the relay ran".to_string()
            })?;
            let Some(reason) = shed_signal.borrow().clone() else {
                return Err("the resolver's ender changed without a reason".to_string());
            };
            return Ok(Copied::Shed(reason));
        }
    };
    // The client is gone before the service's first byte: whether it sent a
    // request or nothing at all, no answer has anybody to go to.
    if client_ended && !hold.answer_began.load(Ordering::Acquire) {
        hold.abandoned();
        return Ok(Copied::Done);
    }
    match client_ended {
        true => download.await.map(|_| ()).map_err(failed)?,
        false => upload.await.map(|_| ()).map_err(failed)?,
    }
    Ok(Copied::Done)
}
