//! Telling a client the upstream is unreachable, in words it can read.
//!
//! A connection that simply closes reports a transport error naming neither the
//! proxy nor the service, so every refused client is answered with an HTTP 502
//! whose body names the service, the endpoint and the cause, then closed.

use tokio::io::{AsyncWrite, AsyncWriteExt};

use crate::service_resolution::ResolverAdapter;

/// Surface an unreachable upstream instead of letting the client hang: one
/// line naming the service, endpoint, and cause in the served log, the same
/// sentence to the client as an HTTP 502, and a close. A refused connection is
/// a handled outcome, so callers return `Ok(())` rather than doubling the line
/// through `serve_adapter`.
pub(super) async fn refuse_connection<W>(
    writer: &mut W,
    adapter: &ResolverAdapter,
    endpoint: &str,
    cause: &str,
) where
    W: AsyncWrite + Unpin,
{
    let sentence = format!(
        "stado resolver service={} consumer={} endpoint={} refused connection: {}",
        adapter.service, adapter.consumer, endpoint, cause
    );
    eprintln!("{sentence}");
    let body = format!("upstream unavailable: {sentence}\n");
    let answer = format!(
        "HTTP/1.1 502 Bad Gateway\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = writer.write_all(answer.as_bytes()).await;
    let _ = writer.shutdown().await;
}
