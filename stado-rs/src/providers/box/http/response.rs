//! Response handling: whole-body reads, JSON parsing, and error mapping.
//!
//! Python `_read_bounded`, `_parse_json`, `_raise_http_error`, and the
//! redacted class-name-only transport failure, all consumed by the
//! `request` sibling.

use serde_json::{Map, Value};

use super::super::types::{
    first_truthy_str, required_dict, safe_text, transient_status, BoxApiError, BoxError,
};

/// The whole response body.
pub(super) async fn read_body(response: reqwest::Response) -> Result<Vec<u8>, BoxError> {
    Ok(response.bytes().await.map_err(transport_error)?.to_vec())
}

/// Python `_parse_json`: empty body -> {}; non-object or invalid JSON is a
/// transport failure.
pub(super) fn parse_json(raw: &[u8]) -> Result<Map<String, Value>, BoxError> {
    if raw.is_empty() {
        return Ok(Map::new());
    }
    let text =
        std::str::from_utf8(raw).map_err(|_| BoxError::transport("Boat returned invalid JSON"))?;
    let value: Value = serde_json::from_str(text)
        .map_err(|_| BoxError::transport("Boat returned invalid JSON"))?;
    required_dict(value, "JSON")
}

/// Python `_raise_http_error`: code/message resolve payload -> nested error
/// object -> defaults.
pub(super) fn api_error(status: u16, raw: &[u8]) -> BoxError {
    let parsed = if raw.is_empty() {
        Ok(Value::Object(Map::new()))
    } else {
        serde_json::from_slice::<Value>(raw)
    };
    // A body that is not a JSON object names no code or message; the
    // sentence below then carries the body and why it could not be read.
    let (payload, unreadable) = match parsed {
        Ok(Value::Object(object)) => (object, String::new()),
        Ok(other) => (
            Map::new(),
            format!(" (the body is JSON but not an object: {other})"),
        ),
        Err(error) => (Map::new(), format!(" (the body is not JSON: {error})")),
    };
    let nested = payload.get("error").and_then(Value::as_object);
    let code = first_truthy_str(
        &[payload.get("code"), nested.and_then(|e| e.get("code"))],
        "http_error",
    );
    // Boat's own body when it names no message: a sentence written here would
    // read as Boat's, and a body that is not JSON would be dropped whole.
    let unnamed = format!(
        "Boat answered HTTP {status} without a message: {}{unreadable}",
        String::from_utf8_lossy(raw).trim()
    );
    let message = first_truthy_str(
        &[
            payload.get("message"),
            nested.and_then(|e| e.get("message")),
        ],
        &unnamed,
    );
    let request_id = first_truthy_str(&[payload.get("requestId")], "");
    BoxApiError::new(
        status,
        &code,
        &message,
        &request_id,
        transient_status(status),
    )
    .into()
}

/// Python `except (URLError, OSError)`: redacted, class-name-only transport
/// failure.
pub(super) fn transport_error(err: reqwest::Error) -> BoxError {
    let kind = if err.is_connect() {
        "connect_error"
    } else {
        "transport_error"
    };
    BoxError::transport(format!(
        "Boat transport failed: {}",
        safe_text(kind, "transport_error")
    ))
}
