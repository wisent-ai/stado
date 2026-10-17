//! Reading workload output back out of the Boat sandbox: the file-response
//! envelope and the prompt/response event listings, read to their last page.

use serde_json::{Map, Value};

use crate::providers::r#box::{BoxClient, BoxError};

use super::super::runtime::Keepalive;
use super::super::BoxDispatchError;

/// Unwrap the nested `{"file": {...}}` envelope and require string content.
pub fn file_content(value: &Map<String, Value>) -> Result<String, BoxError> {
    let nested = match value.get("file") {
        Some(Value::Object(file)) => file,
        _ => value,
    };
    match nested.get("content") {
        Some(Value::String(content)) => Ok(content.clone()),
        Some(other) => Err(BoxError::transport(format!(
            "Boat answered a file read with content that is not text: {other}"
        ))),
        None => Err(BoxError::transport(
            "Boat answered a file read without its content",
        )),
    }
}

/// The text field `key` of an event, when the event carries it as text.
fn text<'a>(object: &'a Map<String, Value>, key: &str) -> Option<&'a str> {
    object.get(key).and_then(Value::as_str)
}

/// Whether the event's data marks it with the flag `key`; an event that
/// does not carry the flag is not marked.
fn flagged(data: &Map<String, Value>, key: &str) -> bool {
    matches!(data.get(key), Some(Value::Bool(true)))
}

/// The id Boat gave the prompt whose text starts with `marker`, read from
/// every prompt event; `None` when the sandbox's whole prompt log carries no
/// prompt with that marker.
pub(crate) async fn recover_prompt_id(
    client: &BoxClient,
    box_id: &str,
    marker: &str,
    keepalive: &mut Keepalive<'_, '_>,
) -> Result<Option<String>, BoxDispatchError> {
    let mut cursor = String::new();
    loop {
        let page = client.list_events(box_id, &cursor, "asc", "prompt").await?;
        keepalive.ping().await?;
        for event in &page.events {
            let Some(Value::Object(data)) = event.get("data") else {
                continue;
            };
            let marked = text(data, "prompt").is_some_and(|prompt| prompt.starts_with(marker));
            if text(event, "type") != Some("prompt") || !marked {
                continue;
            }
            let id = text(event, "taskId")
                .filter(|id| !id.is_empty())
                .or_else(|| text(event, "id"))
                .filter(|id| !id.is_empty());
            return match id {
                Some(id) => Ok(Some(id.to_string())),
                None => Err(BoxDispatchError::runtime(format!(
                    "the Boat prompt event marked {marker:?} in {box_id} names neither taskId nor id"
                ))),
            };
        }
        if !page.has_more || page.next_cursor.is_empty() {
            return Ok(None);
        }
        cursor = page.next_cursor;
    }
}

/// Every final response-event content of the prompt, joined by newlines.
pub(crate) async fn prompt_output(
    client: &BoxClient,
    box_id: &str,
    prompt_id: &str,
    keepalive: &mut Keepalive<'_, '_>,
) -> Result<String, BoxDispatchError> {
    let mut cursor = String::new();
    let mut parts: Vec<String> = Vec::new();
    loop {
        let page = client
            .list_events(box_id, &cursor, "asc", "response")
            .await?;
        keepalive.ping().await?;
        for event in &page.events {
            let Some(Value::Object(data)) = event.get("data") else {
                continue;
            };
            let Some(content) = text(data, "content") else {
                continue;
            };
            if text(event, "type") != Some("response")
                || text(event, "taskId") != Some(prompt_id)
                || flagged(data, "is_streaming")
                || flagged(data, "is_reverted")
            {
                continue;
            }
            parts.push(content.to_string());
        }
        if !page.has_more || page.next_cursor.is_empty() {
            break;
        }
        cursor = page.next_cursor;
    }
    Ok(parts.join("\n"))
}
