//! When a connection has stopped being used, and how it is ended.
//!
//! A request/response connection is silent in one direction for as long as the
//! service is working, so a per-direction idle timer is not a measure of a dead
//! connection at all: it is a cap on how long an answer may take, and when it
//! fires it truncates an answer that was still arriving. A connection is idle
//! only when neither direction has moved a byte inside the window.

use std::time::Duration;

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use super::refusal::http_request_head;

/// One connection's last activity, in milliseconds since the proxy accepted
/// it, shared by both directions.
///
/// A request/response connection is silent in one direction for as long as
/// the service works, so a per-direction idle timer is not a measure of a
/// dead connection at all: it is a cap on how long an answer may take, and
/// when it fires the proxy shuts the half it is copying into, truncating an
/// answer that was still arriving. A client reads that as `connection closed
/// before message completed`, which names neither the proxy nor the service
/// that was working. A connection is idle when NEITHER direction has moved a
/// byte inside the window; the retention bound the window exists for is
/// unchanged, because a connection nobody is using is still closed after it.
pub(super) struct Activity {
    started: std::time::Instant,
    last_millis: std::sync::atomic::AtomicU64,
    /// The client's first bytes were an HTTP request, so a refusal written
    /// back to it is a message it can read rather than noise in a protocol
    /// this proxy knows nothing about.
    client_spoke_http: std::sync::atomic::AtomicBool,
}

impl Activity {
    pub(super) fn new() -> Self {
        Self {
            started: std::time::Instant::now(),
            last_millis: std::sync::atomic::AtomicU64::new(0),
            client_spoke_http: std::sync::atomic::AtomicBool::new(false),
        }
    }

    fn touch(&self) {
        self.last_millis.store(
            self.started.elapsed().as_millis() as u64,
            std::sync::atomic::Ordering::Relaxed,
        );
    }

    /// How long ago either direction last moved a byte.
    fn since(&self) -> Duration {
        let last = self.last_millis.load(std::sync::atomic::Ordering::Relaxed);
        self.started
            .elapsed()
            .saturating_sub(Duration::from_millis(last))
    }

    fn saw_request(&self, head: &[u8]) {
        if http_request_head(head) {
            self.client_spoke_http
                .store(true, std::sync::atomic::Ordering::Relaxed);
        }
    }

    fn spoke_http(&self) -> bool {
        self.client_spoke_http
            .load(std::sync::atomic::Ordering::Relaxed)
    }
}

/// What one direction did, so a cut can be reported with the bytes behind it.
pub(super) struct Transfer {
    pub(super) bytes: u64,
    pub(super) cut: bool,
}

pub(super) async fn copy_until_idle<R, W>(
    reader: &mut R,
    writer: &mut W,
    idle: Duration,
    activity: &Activity,
    on_cut: Option<&[u8]>,
) -> Result<Transfer, std::io::Error>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let mut buffer = [0_u8; 16 * 1024];
    let mut bytes = 0_u64;
    loop {
        let read = match tokio::time::timeout(idle, reader.read(&mut buffer)).await {
            Ok(result) => result?,
            Err(_) => {
                // Silent in this direction only: the other one is moving, so
                // the connection is in use and this is not the socket the
                // window exists to reclaim.
                if activity.since() < idle {
                    continue;
                }
                return cut(writer, bytes, on_cut, activity).await;
            }
        };
        if read == 0 {
            writer.shutdown().await?;
            return Ok(Transfer { bytes, cut: false });
        }
        activity.saw_request(&buffer[..read]);
        activity.touch();
        match tokio::time::timeout(idle, writer.write_all(&buffer[..read])).await {
            Ok(result) => result?,
            Err(_) => {
                if activity.since() < idle {
                    continue;
                }
                return cut(writer, bytes, on_cut, activity).await;
            }
        }
        bytes = bytes.saturating_add(read as u64);
        activity.touch();
    }
}

/// End one direction on the idle window, telling the reader why when it has
/// received nothing yet.
///
/// A client whose connection simply closes reports a transport error — `error
/// sending request ...: connection closed before message completed` — and
/// nothing in that sentence names the proxy, the window or the service that
/// did not answer, which leaves the cause to be found by reading this file.
/// Once a byte of the answer has already been forwarded the message cannot be
/// retracted, so the close is all that is left and the served log carries the
/// sentence.
async fn cut<W>(
    writer: &mut W,
    bytes: u64,
    on_cut: Option<&[u8]>,
    activity: &Activity,
) -> Result<Transfer, std::io::Error>
where
    W: AsyncWrite + Unpin,
{
    if bytes == 0 && activity.spoke_http() {
        if let Some(message) = on_cut {
            let _ = writer.write_all(message).await;
        }
    }
    writer.shutdown().await?;
    Ok(Transfer { bytes, cut: true })
}
