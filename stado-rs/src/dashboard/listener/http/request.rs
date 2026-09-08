//! One parsed request: the bounded head, the Content-Length-framed body, and
//! the bytes carried over to the next request on a reused connection.

use tokio::io::AsyncReadExt;
use tokio::net::TcpStream;

use crate::dashboard::operator_console;

// ---------------------------------------------------------------------------
// Minimal hand-rolled HTTP/1.1 (no framework dependency, per the port spec)
// ---------------------------------------------------------------------------

/// Request head cap (Python's http.server parses a similar 64 KiB budget).
pub(crate) const MAX_HEAD_BYTES: usize = 65536;
/// Desktop and API imports are bounded independently from ordinary JSON
/// controls; the CLI reads local files directly and has no transport envelope.
const MAX_REGISTRY_IMPORT_BYTES: usize = 2 * 1024 * 1024;

pub(crate) struct Request {
    pub(crate) method: String,
    /// Raw request target including the query string (Python `self.path`).
    pub(crate) path: String,
    /// Exactly as it appeared on the request line, because it decides whether
    /// this connection may be reused when the request says nothing.
    pub(crate) version: String,
    /// Lowercased names with trimmed values.
    pub(crate) headers: Vec<(String, String)>,
    pub(crate) content_length: usize,
    pub(crate) body: Vec<u8>,
    /// Connection peer, filled in by the accept path. `None` when the socket
    /// no longer has one to report.
    pub(crate) peer: Option<std::net::IpAddr>,
}

impl Request {
    pub(crate) fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    }

    /// HTTP/1.1 keeps a connection open unless the request asks otherwise;
    /// HTTP/1.0 keeps it only when the request asks for it by name.
    pub(crate) fn wants_keep_alive(&self) -> bool {
        let connection = self
            .header("connection")
            .unwrap_or_default()
            .to_ascii_lowercase();
        let mut tokens = connection.split(',').map(str::trim);
        if tokens.clone().any(|token| token == "close") {
            return false;
        }
        self.version == "HTTP/1.1" || tokens.any(|token| token == "keep-alive")
    }
}

pub(crate) fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

/// Read one request with a bounded head and body. Body framing is deliberately
/// limited to Content-Length; mutating routes reject Transfer-Encoding.
///
/// `head_idle` bounds the wait for a complete head, and nothing else: the head
/// is what an idle or abandoned connection is failing to send, while a body
/// still arriving is a transfer that may legitimately outlast any idle limit.
/// A connection that goes quiet before saying anything reads as a clean close,
/// the same as an EOF between requests.
pub(crate) async fn read_request(
    stream: &mut TcpStream,
    carry: &mut Vec<u8>,
    head_idle: std::time::Duration,
) -> std::io::Result<Option<Request>> {
    // Whatever the previous request on this connection read past its own body.
    let mut buf: Vec<u8> = std::mem::take(carry);
    let mut tmp = [0u8; 8192];
    let head_deadline = tokio::time::Instant::now() + head_idle;
    let head_end = loop {
        // Check before reading: a pipelined head may already be complete in
        // the bytes carried over, and blocking on the socket would deadlock
        // against a client waiting for its answer.
        if let Some(pos) = find_subslice(&buf, b"\r\n\r\n") {
            break pos;
        }
        if buf.len() > MAX_HEAD_BYTES {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "HTTP request head too large",
            ));
        }
        let n = match tokio::time::timeout_at(head_deadline, stream.read(&mut tmp)).await {
            Ok(read) => read?,
            Err(_) if buf.is_empty() => return Ok(None),
            Err(_) => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    "HTTP request head timed out",
                ));
            }
        };
        if n == 0 {
            if buf.is_empty() {
                return Ok(None);
            }
            return Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "incomplete HTTP request head",
            ));
        }
        buf.extend_from_slice(&tmp[..n]);
    };
    let head = String::from_utf8_lossy(&buf[..head_end]).into_owned();
    let mut lines = head.split("\r\n");
    let request_line = lines.next().unwrap_or("");
    let mut parts = request_line.split_whitespace();
    let (method, path, version) = match (parts.next(), parts.next(), parts.next()) {
        (Some(method), Some(path), Some(version)) if version.starts_with("HTTP/") => {
            (method.to_string(), path.to_string(), version.to_string())
        }
        _ => (String::new(), String::new(), String::new()),
    };
    let mut headers = Vec::new();
    for line in lines {
        if let Some((name, value)) = line.split_once(':') {
            let name = name.trim().to_ascii_lowercase();
            if name == "transfer-encoding" {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "Transfer-Encoding is unsupported",
                ));
            }
            if name == "content-length" && headers.iter().any(|(key, _)| key == &name) {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "duplicate Content-Length",
                ));
            }
            headers.push((name, value.trim().to_string()));
        }
    }
    let body_start = head_end + b"\r\n\r\n".len();
    let content_length_header = headers
        .iter()
        .find(|(name, _)| name == "content-length")
        .map(|(_, value)| value.as_str());
    let object_put = method == "PUT" && path.starts_with("/api/object?");
    let registry_import = method == "POST"
        && path
            .split_once('?')
            .map_or(path.as_str(), |(route, _)| route)
            == "/api/registry/import";
    let content_length = match content_length_header {
        Some(value) => value.parse::<usize>().map_err(|_| {
            std::io::Error::new(std::io::ErrorKind::InvalidData, "invalid Content-Length")
        })?,
        None if object_put || registry_import => {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "mutating object and registry import requests require Content-Length",
            ));
        }
        None => usize::default(),
    };
    let max_body_bytes = if object_put {
        crate::object_store::max_object_bytes()
    } else if method == "POST" && path == "/api/operator/run" {
        operator_console::MAX_REQUEST_BYTES
    } else if registry_import {
        MAX_REGISTRY_IMPORT_BYTES
    } else {
        MAX_HEAD_BYTES
    };
    if content_length > max_body_bytes {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "HTTP request body too large",
        ));
    }
    let available = buf.len().saturating_sub(body_start).min(content_length);
    let mut body = Vec::with_capacity(content_length);
    body.extend_from_slice(&buf[body_start..body_start + available]);
    if body.len() < content_length {
        let received = body.len();
        body.resize(content_length, 0);
        stream.read_exact(&mut body[received..]).await?;
    }
    // Anything beyond this request's body belongs to the next one on this
    // connection, not to this buffer.
    let consumed = body_start.saturating_add(content_length);
    if buf.len() > consumed {
        carry.extend_from_slice(&buf[consumed..]);
    }
    Ok(Some(Request {
        method,
        path,
        version,
        headers,
        content_length,
        body,
        peer: None,
    }))
}
