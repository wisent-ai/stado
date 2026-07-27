//! Test-only helpers: a minimal loopback HTTP/1.1 server that plays back
//! canned responses and records the raw requests it received. Used by the
//! provider transport tests (box, vast) so no test needs live network.

use std::sync::{Arc, Mutex};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

/// Serializes tests that mutate process-global state (environment
/// variables, the registry downloader override in `targets.rs` plus its
/// TTL cache). Tokio-aware so the guard can be held across `.await`.
pub static GLOBAL_STATE_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Handle to a running playback server.
pub struct MockHttp {
    /// `http://127.0.0.1:<port>` — pass to the transport under test.
    pub base_url: String,
    /// Every raw request head+body received, in order.
    pub requests: Arc<Mutex<Vec<String>>>,
    handle: tokio::task::JoinHandle<()>,
}

impl MockHttp {
    /// Stop the server task (responses not yet consumed are dropped).
    pub fn stop(&self) {
        self.handle.abort();
    }
}

/// Build a complete HTTP/1.1 response with a JSON content type.
pub fn http_response(status: u16, reason: &str, body: &str) -> String {
    format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        body.len(),
        body
    )
}

fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

/// Bind a loopback port and serve exactly `responses.len()` requests, one
/// canned response per accepted connection (HTTP/1.0 style: a client that
/// reuses a connection simply opens a new one, which reqwest does when the
/// previous response carried `Connection: close`).
pub async fn mock_http(responses: Vec<String>) -> MockHttp {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind loopback");
    let addr = listener.local_addr().expect("local addr");
    let requests: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let recorded = Arc::clone(&requests);
    let handle = tokio::spawn(async move {
        for response in responses {
            let Ok((mut socket, _)) = listener.accept().await else {
                return;
            };
            let mut buf: Vec<u8> = Vec::new();
            let mut tmp = [0u8; 8192];
            let request = loop {
                let Ok(n) = socket.read(&mut tmp).await else {
                    return;
                };
                if n == 0 {
                    break String::from_utf8_lossy(&buf).into_owned();
                }
                buf.extend_from_slice(&tmp[..n]);
                if let Some(head_end) = find_subslice(&buf, b"\r\n\r\n") {
                    let head = String::from_utf8_lossy(&buf[..head_end]).into_owned();
                    let content_length = head
                        .lines()
                        .filter_map(|line| line.split_once(':'))
                        .find(|(name, _)| name.trim().eq_ignore_ascii_case("content-length"))
                        .and_then(|(_, value)| value.trim().parse::<usize>().ok())
                        .unwrap_or(0);
                    if buf.len() >= head_end + 4 + content_length {
                        break String::from_utf8_lossy(&buf[..head_end + 4 + content_length])
                            .into_owned();
                    }
                }
            };
            recorded.lock().expect("requests lock").push(request);
            if socket.write_all(response.as_bytes()).await.is_err() {
                return;
            }
            let _ = socket.shutdown().await;
        }
    });
    MockHttp {
        base_url: format!("http://{addr}"),
        requests,
        handle,
    }
}
