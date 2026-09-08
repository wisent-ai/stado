//! Box file, artifact, and event payload readers.
//!
//! Port of the file/artifact/event half of `stado/providers/box/client.py`.

use serde_json::{json, Map, Value};

use super::super::types::{jbool, jstr, BoxError, BoxEventPage};
use super::BoxClient;

impl BoxClient {
    /// GET /boxes/{id}/files.
    pub async fn read_file(
        &self,
        box_id: &str,
        path: &str,
        encoding: &str,
    ) -> Result<Map<String, Value>, BoxError> {
        self.transport
            .request_json(
                "GET",
                &format!("{}/files", Self::box_path(box_id)?),
                None,
                &[
                    ("path", path.to_string()),
                    ("encoding", encoding.to_string()),
                ],
                &["file.read"],
            )
            .await
    }

    /// PUT /boxes/{id}/files.
    pub async fn write_file(
        &self,
        box_id: &str,
        path: &str,
        content: &str,
        encoding: &str,
    ) -> Result<Map<String, Value>, BoxError> {
        let body = json!({"path": path, "content": content, "encoding": encoding});
        self.transport
            .request_json(
                "PUT",
                &format!("{}/files", Self::box_path(box_id)?),
                Some(&body),
                &[],
                &["file.written", "file.write"],
            )
            .await
    }

    /// GET /boxes/{id}/artifacts (binary; `max_bytes` must be positive —
    /// Python `ValueError`).
    pub async fn download_artifact(
        &self,
        box_id: &str,
        path: &str,
        max_bytes: usize,
    ) -> Result<Vec<u8>, BoxError> {
        if max_bytes == 0 {
            return Err(BoxError::value("artifact max_bytes must be positive"));
        }
        self.transport
            .request_binary(
                "GET",
                &format!("{}/artifacts", Self::box_path(box_id)?),
                &[("path", path.to_string())],
                max_bytes,
            )
            .await
    }

    /// GET /boxes/{id}/events.
    pub async fn list_events(
        &self,
        box_id: &str,
        cursor: &str,
        limit: i64,
        sort: &str,
        event_type: &str,
    ) -> Result<BoxEventPage, BoxError> {
        let value = self
            .transport
            .request_json(
                "GET",
                &format!("{}/events", Self::box_path(box_id)?),
                None,
                &[
                    ("cursor", cursor.to_string()),
                    ("limit", limit.to_string()),
                    ("sort", sort.to_string()),
                    ("type", event_type.to_string()),
                ],
                &["events.list"],
            )
            .await?;
        let events_value = value
            .get("events")
            .and_then(Value::as_array)
            .ok_or_else(|| BoxError::transport("Box events response has invalid events"))?;
        let mut events = Vec::with_capacity(events_value.len());
        for event in events_value {
            events.push(
                event
                    .as_object()
                    .cloned()
                    .ok_or_else(|| BoxError::transport("Box events response has invalid events"))?,
            );
        }
        let page = value.get("pageInfo").and_then(Value::as_object);
        Ok(BoxEventPage {
            events,
            next_cursor: page.map(|p| jstr(p.get("nextCursor"))).unwrap_or_default(),
            has_more: page.is_some_and(|p| jbool(p.get("hasMore"))),
        })
    }
}
