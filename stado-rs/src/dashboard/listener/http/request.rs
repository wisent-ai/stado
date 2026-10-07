//! One parsed request: the bounded head, the Content-Length-framed body, and
//! the bytes carried over to the next request on a reused connection.

use std::{env::VarError, num::NonZeroUsize};

use serde::Deserialize;
use serde_json::Value;

use crate::capabilities::DASHBOARD_REQUEST_LIMITS_CONFIG;
use crate::dashboard::DashboardError;

use tokio::io::AsyncReadExt;
use tokio::net::TcpStream;

use crate::dashboard::operator_console;

// ---------------------------------------------------------------------------
// Minimal hand-rolled HTTP/1.1 (no framework dependency, per the port spec)
// ---------------------------------------------------------------------------

/// Deployment-owned bounds, checked before the listener accepts traffic.
#[derive(Clone, Copy, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RequestLimits {
    pub(crate) head_bytes: NonZeroUsize,
    pub(crate) body_bytes: NonZeroUsize,
    pub(crate) registry_import_bytes: NonZeroUsize,
}

impl RequestLimits {
    pub(crate) fn parse(value: Value) -> Result<Self, String> {
        serde_json::from_value(value).map_err(|error| {
            format!(
                "{} must declare positive whole-byte head_bytes, body_bytes and registry_import_bytes: {error}",
                DASHBOARD_REQUEST_LIMITS_CONFIG.path
            )
        })
    }

    pub(crate) fn read() -> Result<Self, DashboardError> {
        let field = &DASHBOARD_REQUEST_LIMITS_CONFIG;
        let value = match std::env::var(field.env) {
            Ok(raw) => serde_json::from_str(&raw).map_err(|error| {
                DashboardError::Refused(format!(
                    "{} cannot be read as {} JSON: {error}",
                    field.env, field.path
                ))
            })?,
            Err(VarError::NotPresent) => crate::config_file::field_value(field).ok_or_else(|| {
                DashboardError::Refused(format!(
                    "API request limits are not declared: set {} with stado config set, or {} with its JSON document; head_bytes, body_bytes and registry_import_bytes are required",
                    field.path, field.env
                ))
            })?,
            Err(error) => {
                return Err(DashboardError::Refused(format!(
                    "{} cannot be read: {error}",
                    field.env
                )))
            }
        };
        Self::parse(value).map_err(DashboardError::Refused)
    }
}

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
    pub(crate) head_limit: usize,
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
    let mut tmp = [0u8; 8192];
    let head_end = loop {
        // Check before reading: a pipelined head may already be complete in
        // the bytes carried over, and blocking on the socket would deadlock
        // against a client waiting for its answer.
        if let Some(pos) = find_subslice(&buf, b"\r\n\r\n") {
            // Count only the head, including its terminator. This read may
            // also contain a body or the next request on the connection.
            if pos + b"\r\n\r\n".len() > limits.head_bytes.get() {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "HTTP request head too large",
                ));
            }
            break pos;
        }
        if buf.len() >= limits.head_bytes.get() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "HTTP request head too large",
            ));
        }
        let n = stream.read(&mut tmp).await?;
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
    let route = path
        .split_once('?')
        .map_or(path.as_str(), |(route, _)| route);
    let object_put = method == "PUT" && path.starts_with("/api/object?");
    // A compose names every part of one multipart upload, so its body grows
    // with the object: a large source archive's part list measured 227192
    // bytes, and the 64 KiB head cap refused it after every part had been
    // uploaded. It is the object writer's request, sized by the object, the
    // same as an object PUT.
    let object_compose = method == "POST" && route == "/api/object/compose";
    let registry_import = method == "POST" && route == "/api/registry/import";
    let content_length = match content_length_header {
        Some(value) => value.parse::<usize>().map_err(|_| {
            std::io::Error::new(std::io::ErrorKind::InvalidData, "invalid Content-Length")
        })?,
        None if object_put || object_compose || registry_import => {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "mutating object and registry import requests require Content-Length",
            ));
        }
        None => 0,
    };
    // An object PUT or compose carries whatever the authenticated writer
    // stores; the object API sets no size of its own on it.
    let max_body_bytes = if object_put || object_compose {
        None
    } else if method == "POST" && path == "/api/operator/run" {
        Some(operator_console::MAX_REQUEST_BYTES)
    } else if registry_import {
        Some(limits.registry_import_bytes.get())
    } else {
        Some(limits.body_bytes.get())
    };
    if let Some(max_body_bytes) = max_body_bytes.filter(|max| content_length > *max) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!(
                "{method} {route} declares a {content_length}-byte body; this route accepts at most {max_body_bytes} bytes"
            ),
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
        head_limit: limits.head_bytes.get(),
    }))
}
