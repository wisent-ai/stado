//! Response handling: whole-body reads, JSON parsing, and error mapping.
//!
//! Python `_read_bounded`, `_parse_json`, `_raise_http_error`, and the
//! redacted class-name-only transport failure, all consumed by the
//! `request` sibling.

use serde_json::{Map, Value};

use super::super::types::{
    first_truthy_str, required_dict, safe_text, BoxApiError, BoxError, TRANSIENT_HTTP,
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
        std::str::from_utf8(raw).map_err(|_| BoxError::transport("Box returned invalid JSON"))?;
    let value: Value =
        serde_json::from_str(text).map_err(|_| BoxError::transport("Box returned invalid JSON"))?;
    required_dict(value, "JSON")
}

/// Python `_raise_http_error`: code/message resolve payload -> nested error
/// object -> defaults.
pub(super) fn api_error(status: u16, raw: &[u8]) -> BoxError {
    let payload: Map<String, Value> = if raw.is_empty() {
        Map::new()
    } else {
        std::str::from_utf8(raw)
            .ok()
            .and_then(|text| serde_json::from_str::<Value>(text).ok())
            .and_then(|value| value.as_object().cloned())
            .unwrap_or_default()
    };
    let nested = payload.get("error").and_then(Value::as_object);
    let code = first_truthy_str(
        &[payload.get("code"), nested.and_then(|e| e.get("code"))],
        "http_error",
    );
    let message = first_truthy_str(
        &[
            payload.get("message"),
            nested.and_then(|e| e.get("message")),
        ],
        "Box API request failed",
    );
    let request_id = first_truthy_str(&[payload.get("requestId")], "");
    BoxApiError::new(
        status,
        &code,
        &message,
        &request_id,
        TRANSIENT_HTTP.contains(&status),
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
        "Box transport failed: {}",
        safe_text(kind, "transport_error")
    ))
}
