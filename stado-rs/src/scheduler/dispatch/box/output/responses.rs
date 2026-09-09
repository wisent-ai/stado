//! Reading workload output back out of the Box: the file-response envelope
//! and the bounded prompt/response event paginations.
//!
//! Port of `stado/scheduler/dispatch/box/output.py`.

use serde_json::{Map, Value};

use crate::providers::r#box::{BoxClient, BoxError};

use super::super::runtime::Keepalive;
use super::super::BoxDispatchError;
use super::{EVENT_LIMIT, EVENT_PAGES, LOG_BYTES};

/// Python `file_content`: unwrap the nested `{"file": {...}}` envelope and
/// require string content.
pub fn file_content(value: &Map<String, Value>) -> Result<String, BoxError> {
    let nested = match value.get("file") {
        Some(Value::Object(file)) => file,
        _ => value,
    };
    match nested.get("content") {
        Some(Value::String(content)) => Ok(content.clone()),
        _ => Err(BoxError::transport(
            "Box file response omitted string content",
        )),
    }
}

/// Python `recover_prompt_id`: scan prompt events for the operation marker.
pub(crate) async fn recover_prompt_id(
    client: &BoxClient,
    box_id: &str,
    marker: &str,
    keepalive: &mut Keepalive<'_, '_>,
) -> Result<String, BoxDispatchError> {
    let mut cursor = String::new();
    for _ in 0..EVENT_PAGES {
        let page = client
            .list_events(box_id, &cursor, EVENT_LIMIT, "asc", "prompt")
            .await?;
        keepalive.ping().await?;
        for event in &page.events {
            let empty = Map::new();
            let data = match event.get("data") {
                Some(Value::Object(data)) => data,
                _ => &empty,
            };
            let prompt = data.get("prompt").and_then(Value::as_str).unwrap_or("");
            if event.get("type").and_then(Value::as_str) == Some("prompt")
                && prompt.starts_with(marker)
            {
                let id = event
                    .get("taskId")
                    .and_then(Value::as_str)
                    .filter(|s| !s.is_empty())
                    .or_else(|| event.get("id").and_then(Value::as_str))
                    .unwrap_or("");
                return Ok(id.to_string());
            }
        }
        if !page.has_more || page.next_cursor.is_empty() {
            break;
        }
        cursor = page.next_cursor;
    }
    Ok(String::new())
}

/// Python `prompt_output`: join response-event contents bounded to
/// LOG_BYTES (byte-exact truncation with lossy UTF-8 decode).
pub(crate) async fn prompt_output(
    client: &BoxClient,
    box_id: &str,
    prompt_id: &str,
    keepalive: &mut Keepalive<'_, '_>,
) -> Result<String, BoxDispatchError> {
    let mut cursor = String::new();
    let mut parts: Vec<String> = Vec::new();
    let mut size = 0usize;
    for _ in 0..EVENT_PAGES {
        let page = client
            .list_events(box_id, &cursor, EVENT_LIMIT, "asc", "response")
            .await?;
        keepalive.ping().await?;
        for event in &page.events {
            let empty = Map::new();
            let data = match event.get("data") {
                Some(Value::Object(data)) => data,
                _ => &empty,
            };
            let Some(content) = data.get("content").and_then(Value::as_str) else {
                continue;
            };
            if event.get("type").and_then(Value::as_str) != Some("response")
                || event.get("taskId").and_then(Value::as_str) != Some(prompt_id)
                || data
                    .get("is_streaming")
                    .and_then(Value::as_bool)
                    .unwrap_or(false)
                || data
                    .get("is_reverted")
                    .and_then(Value::as_bool)
                    .unwrap_or(false)
            {
                continue;
            }
            let encoded = content.as_bytes();
            let remaining = LOG_BYTES - size;
            if remaining == 0 {
                return Ok(parts.join("\n"));
            }
            let take = encoded.len().min(remaining);
            parts.push(String::from_utf8_lossy(&encoded[..take]).into_owned());
            size += take;
        }
        if !page.has_more || page.next_cursor.is_empty() {
            break;
        }
        cursor = page.next_cursor;
    }
    Ok(parts.join("\n"))
}
