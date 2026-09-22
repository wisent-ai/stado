//! Telling a client the upstream is unreachable, in a language it can read.
//!
//! A connection that simply closes reports a transport error naming neither the
//! proxy nor the service, so a client whose first bytes were an HTTP request is
//! answered with a 502 before the close. A client speaking anything else gets
//! the prompt close alone, because a 502 would be noise in its protocol.

use std::time::Duration;

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use crate::service_resolution::ResolverAdapter;

/// Terse refusal for clients that speak HTTP, sent before the prompt close.
pub(super) const UPSTREAM_REFUSAL: &[u8] = b"HTTP/1.1 502 Bad Gateway\r\nContent-Type: text/plain\r\nContent-Length: 21\r\nConnection: close\r\n\r\nupstream unavailable\n";

/// Brief window to sniff the client's first bytes on a refusal where the
/// establishment phase never read any. The connection is already dead, so the
/// only cost of a miss is closing without the 502 body.
const REFUSAL_SNIFF: Duration = Duration::from_millis(250);

/// The stream speaks HTTP when its first bytes open with a request method.
pub(super) fn http_request_head(head: &[u8]) -> bool {
    const METHODS: [&[u8]; 9] = [
        b"GET ",
        b"POST ",
        b"PUT ",
        b"DELETE ",
        b"HEAD ",
        b"OPTIONS ",
        b"PATCH ",
        b"CONNECT ",
        b"TRACE ",
    ];
    METHODS.iter().any(|method| head.starts_with(method))
}

/// Surface an unreachable upstream instead of letting the client hang: one
/// line naming the service, endpoint, and cause, then an HTTP 502 when the
/// connection's first bytes are an HTTP request, and a prompt close either
/// way. A refused connection is a handled outcome, so callers return `Ok(())`
/// rather than doubling the line through `serve_adapter`.
pub(super) async fn refuse_connection<R, W>(
    reader: &mut R,
    writer: &mut W,
    adapter: &ResolverAdapter,
    endpoint: &str,
    cause: &str,
    head: Option<&[u8]>,
) where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    eprintln!(
        "stado resolver service={} consumer={} endpoint={} refused connection: {}",
        adapter.service, adapter.consumer, endpoint, cause
    );
    let mut sniff = [0_u8; 512];
    let head = match head {
        Some(head) => Some(head),
        None => match tokio::time::timeout(REFUSAL_SNIFF, reader.read(&mut sniff)).await {
            Ok(Ok(read)) if read > 0 => Some(&sniff[..read]),
            _ => None,
        },
    };
    if head.is_some_and(http_request_head) {
        let _ = writer.write_all(UPSTREAM_REFUSAL).await;
    }
    let _ = writer.shutdown().await;
}
