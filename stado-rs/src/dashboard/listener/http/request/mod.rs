//! One parsed request: the bounded head, the Content-Length-framed body, and
//! the bytes carried over to the next request on a reused connection.

use tokio::io::AsyncReadExt;
use tokio::net::TcpStream;

use crate::dashboard::{fleet_join, operator_console};

mod limits;
mod refusal;
pub(crate) use limits::RequestLimits;
pub(crate) use refusal::Refusal;
use refusal::{content_too_large, length_required, malformed, not_implemented};

// ---------------------------------------------------------------------------
// Minimal hand-rolled HTTP/1.1 (no framework dependency, per the port spec)
// ---------------------------------------------------------------------------

pub(crate) struct Request {
    pub(crate) method: String,
    /// Raw request target including the query string (Python `self.path`).
    pub(crate) path: String,
    /// Exactly as it appeared on the request line, because it decides whether
    /// this connection may be reused when the request says nothing.
    pub(crate) version: String,
    /// Lowercased names with trimmed values.
    pub(crate) headers: Vec<(String, String)>,
    /// Exactly the Content-Length the head declared; none declared is none.
    pub(crate) body: Vec<u8>,
    pub(crate) head_limit: usize,
    pub(crate) console_limits: operator_console::Limits,
    pub(crate) join_limits: fleet_join::Limits,
}

impl Request {
    /// The request target's route and query: everything before the first `?`
    /// and everything after it; a target without one has an empty query
    /// (RFC 3986 §3.4).
    pub(crate) fn target(&self) -> (&str, &str) {
        split_target(&self.path)
    }

    pub(crate) fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    }

    /// HTTP/1.1 keeps a connection open unless the request asks otherwise;
    /// HTTP/1.0 keeps it only when the request asks for it by name.
    pub(crate) fn wants_keep_alive(&self) -> bool {
        let http_1_1 = self.version == "HTTP/1.1";
        let Some(connection) = self.header("connection") else {
            return http_1_1;
        };
        let connection = connection.to_ascii_lowercase();
        let mut tokens = connection.split(',').map(str::trim);
        if tokens.clone().any(|token| token == "close") {
            return false;
        }
        http_1_1 || tokens.any(|token| token == "keep-alive")
    }
}

/// `target` split into its route and its query (see [`Request::target`]).
pub(crate) fn split_target(target: &str) -> (&str, &str) {
    match target.split_once('?') {
        Some(split) => split,
        None => (target, ""),
    }
}

pub(crate) fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

/// The blank line that ends a request head (RFC 9112 §2.1,
/// https://www.rfc-editor.org/rfc/rfc9112#section-2.1).
const HEAD_END: &[u8] = b"\r\n\r\n";

/// RFC 9112 §6.3 (https://www.rfc-editor.org/rfc/rfc9112#section-6.3): a
/// request with neither Content-Length nor Transfer-Encoding has a body of
/// length zero.
const NO_BODY: usize = 0;

/// A head past the bound, named with the bound and where it comes from.
fn head_too_large(what: String, bound: usize) -> std::io::Error {
    refusal::head_too_large(format!(
        "HTTP request head too large: {what} the {bound}-byte head bound ({}.head_bytes, or this host's available memory when none is declared)",
        crate::capabilities::DASHBOARD_REQUEST_LIMITS_CONFIG.path
    ))
}

/// `<method> <target> HTTP/<version>` (RFC 9112 §3), or a refusal that
/// quotes the line: an empty method and path used to reach the routes as
/// though the client had asked for them.
fn request_line(line: Option<&str>) -> std::io::Result<(String, String, String)> {
    let refused = |line: &str| {
        malformed(format!(
            "HTTP request line {line:?} is not <method> <target> HTTP/<version>"
        ))
    };
    let Some(line) = line else {
        return Err(refused(""));
    };
    let mut parts = line.split_whitespace();
    match (parts.next(), parts.next(), parts.next(), parts.next()) {
        (Some(method), Some(path), Some(version), None) if version.starts_with("HTTP/") => {
            Ok((method.to_string(), path.to_string(), version.to_string()))
        }
        _ => Err(refused(line)),
    }
}

/// Read one request with a bounded head and body. Body framing is deliberately
/// limited to Content-Length; mutating routes reject Transfer-Encoding.
///
/// The connection is read until the head is complete or the client closes
/// it. A close before any byte of a new request is a clean end, the same as
/// an EOF between requests.
pub(crate) async fn read_request(
    stream: &mut TcpStream,
    carry: &mut Vec<u8>,
    limits: RequestLimits,
) -> std::io::Result<Option<Request>> {
    // Whatever the previous request on this connection read past its own body.
    let mut buf: Vec<u8> = std::mem::take(carry);
    let bound = limits.head_bytes.get();
    let head_end = loop {
        // Check before reading: a pipelined head may already be complete in
        // the bytes carried over, and blocking on the socket would deadlock
        // against a client waiting for its answer.
        if let Some(pos) = find_subslice(&buf, HEAD_END) {
            // Count only the head, including its terminator. This read may
            // also contain a body or the next request on the connection.
            let head_bytes = pos + HEAD_END.len();
            if head_bytes > bound {
                return Err(head_too_large(
                    format!("its {head_bytes} bytes pass"),
                    bound,
                ));
            }
            break pos;
        }
        if buf.len() >= bound {
            return Err(head_too_large(
                format!("{} bytes without its end reach", buf.len()),
                bound,
            ));
        }
        // Read into the buffer's own spare room: no second buffer, no copy.
        let before = buf.len();
        stream.read_buf(&mut buf).await?;
        if buf.len() == before {
            if buf.is_empty() {
                return Ok(None);
            }
            // InvalidData, not UnexpectedEof: a client that only closed its
            // sending side still reads why its request was refused.
            return Err(malformed(format!(
                "incomplete HTTP request head: the client closed after {} bytes without the blank line that ends a head",
                buf.len()
            )));
        }
    };
    let head = std::str::from_utf8(&buf[..head_end])
        .map_err(|error| malformed(format!("HTTP request head is not UTF-8: {error}")))?;
    let mut lines = head.split("\r\n");
    let (method, path, version) = request_line(lines.next())?;
    let mut headers = Vec::new();
    for line in lines {
        let Some((name, value)) = line.split_once(':') else {
            return Err(malformed(format!(
                "HTTP header line {line:?} has no colon between a name and a value"
            )));
        };
        let name = name.trim().to_ascii_lowercase();
        if name == "transfer-encoding" {
            return Err(not_implemented(
                "Transfer-Encoding is unsupported: this listener frames a body by Content-Length only",
            ));
        }
        if name == "content-length" && headers.iter().any(|(key, _)| key == &name) {
            return Err(malformed("duplicate Content-Length"));
        }
        headers.push((name, value.trim().to_string()));
    }
    let body_start = head_end + HEAD_END.len();
    let content_length_header = headers
        .iter()
        .find(|(name, _)| name == "content-length")
        .map(|(_, value)| value.as_str());
    let (route, _) = split_target(&path);
    let object_put = method == "PUT" && path.starts_with("/api/object?");
    // A compose names every part of one multipart upload, so its body grows
    // with the object: a large source archive's part list measured 227192
    // bytes, and the 64 KiB head cap refused it after every part had been
    // uploaded. It is the object writer's request, sized by the object, the
    // same as an object PUT.
    let object_compose = method == "POST" && route == "/api/object/compose";
    let registry_import = method == "POST" && route == "/api/registry/import";
    let content_length = match content_length_header {
        Some(value) => value
            .parse::<usize>()
            .map_err(|error| malformed(format!("invalid Content-Length {value:?}: {error}")))?,
        None if object_put || object_compose || registry_import => {
            return Err(length_required(
                "mutating object and registry import requests require Content-Length",
            ));
        }
        None => NO_BODY,
    };
    // An object PUT or compose carries whatever the authenticated writer
    // stores; the object API sets no size of its own on it.
    let max_body_bytes = if object_put || object_compose {
        None
    } else if method == "POST" && path == "/api/operator/run" {
        Some(limits.operator_console.request_bytes.get())
    } else if method == "POST" && route == "/api/fleet/join" {
        Some(limits.fleet_join.request_bytes.get())
    } else if registry_import {
        Some(limits.registry_import_bytes.get())
    } else {
        Some(limits.body_bytes.get())
    };
    if let Some(max_body_bytes) = max_body_bytes.filter(|max| content_length > *max) {
        return Err(content_too_large(format!(
            "{method} {route} declares a {content_length}-byte body; this route accepts at most {max_body_bytes} bytes"
        )));
    }
    let available = buf.len().saturating_sub(body_start).min(content_length);
    let mut body = Vec::with_capacity(content_length);
    body.extend_from_slice(&buf[body_start..body_start + available]);
    let missing = content_length - body.len();
    (&mut *stream)
        .take(missing as u64)
        .read_to_end(&mut body)
        .await?;
    if body.len() < content_length {
        return Err(std::io::Error::new(
            std::io::ErrorKind::UnexpectedEof,
            format!(
                "{method} {route}: the client closed after {} of the {content_length} body bytes its Content-Length declares",
                body.len()
            ),
        ));
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
        body,
        head_limit: limits.head_bytes.get(),
        console_limits: limits.operator_console,
        join_limits: limits.fleet_join,
    }))
}
