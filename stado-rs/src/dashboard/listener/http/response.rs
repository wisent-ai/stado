//! One serialized response, the byte-range answer, and the JSON and error
//! bodies every route on this listener answers with.

use serde_json::{json, Value};

use crate::dashboard::DashboardError;
use crate::queue::submit::json_dumps_sorted_compact;
use crate::queue::StorageError;

use super::request::find_subslice;

pub(crate) struct Response {
    pub(crate) status: u16,
    pub(crate) bytes: Vec<u8>,
}

impl Response {
    pub(crate) fn new(status: u16, reason: &str, content_type: &str, body: &[u8]) -> Self {
        Self::new_with_headers(status, reason, content_type, body, &[])
    }

    pub(crate) fn new_with_headers(
        status: u16,
        reason: &str,
        content_type: &str,
        body: &[u8],
        headers: &[(&str, String)],
    ) -> Self {
        let mut head = format!(
            "HTTP/1.1 {status} {reason}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\n",
            body.len()
        );
        for (name, value) in headers {
            head.push_str(name);
            head.push_str(": ");
            head.push_str(value);
            head.push_str("\r\n");
        }
        head.push_str("Connection: keep-alive\r\n\r\n");
        let mut bytes = head.into_bytes();
        bytes.extend_from_slice(body);
        Self { status, bytes }
    }

    /// Rewrite this already-serialized response to announce a close.
    ///
    /// Responses are built with the reusable form because that is now the
    /// common case; the refusal paths that answer without draining the
    /// declared body cannot be followed by another request on the same
    /// connection and say so here. The literal replaced is the one written
    /// directly above, so this can only match what this type produced.
    pub(crate) fn close_connection(&mut self) {
        const REUSE: &[u8] = b"Connection: keep-alive\r\n";
        const CLOSE: &[u8] = b"Connection: close\r\n";
        if let Some(at) = find_subslice(&self.bytes, REUSE) {
            self.bytes
                .splice(at..at + REUSE.len(), CLOSE.iter().copied());
        }
    }

    fn json(status: u16, body: &str) -> Self {
        let reason = match status {
            200 => "OK",
            401 => "Unauthorized",
            409 => "Conflict",
            500 => "Internal Server Error",
            503 => "Service Unavailable",
            _ => "OK",
        };
        Self::new(status, reason, "application/json", body.as_bytes())
    }
}

pub(crate) fn parse_byte_range(value: &str, length: usize) -> Option<(usize, usize)> {
    if length == usize::default() {
        return None;
    }
    let value = value.strip_prefix("bytes=")?;
    if value.contains(',') {
        return None;
    }
    let (start, end) = value.split_once('-')?;
    let start = start.parse::<usize>().ok()?;
    if start >= length {
        return None;
    }
    let last = length.saturating_sub(usize::from(true));
    let end = if end.is_empty() {
        last
    } else {
        end.parse::<usize>().ok()?.min(last)
    };
    (start <= end).then_some((start, end))
}

pub(crate) fn http_status(value: &str) -> u16 {
    value.parse().expect("static HTTP status is valid")
}

fn storage_error_status(error: &StorageError) -> u16 {
    if matches!(error, StorageError::Io(error) if error.kind() == std::io::ErrorKind::WouldBlock) {
        http_status("503")
    } else {
        http_status("500")
    }
}

pub(crate) fn storage_error_response(error: StorageError) -> Response {
    send_json(
        storage_error_status(&error),
        &json!({"error": error.to_string()}),
    )
}

pub(crate) fn dashboard_error_response(error: DashboardError) -> Response {
    match error {
        DashboardError::Storage(error) => storage_error_response(error),
        other => send_json(http_status("500"), &json!({"error": other.to_string()})),
    }
}

pub(crate) fn empty_response(status: u16, reason: &str) -> Response {
    Response::new(status, reason, "text/plain; charset=utf-8", b"")
}

/// Python `_Handler._send_json`:
/// `json.dumps(payload, default=str, sort_keys=True, separators=(",", ":"))`.
pub(crate) fn send_json(status: u16, payload: &Value) -> Response {
    Response::json(status, &json_dumps_sorted_compact(payload))
}
