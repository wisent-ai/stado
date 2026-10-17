//! Boat file, artifact, and event payload readers.

use serde_json::{json, Map, Value};

use super::super::types::{jbool, jstr, BoxError, BoxEventPage};
use super::BoxClient;

impl BoxClient {
    /// GET /sandboxes/{id}/files.
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

    /// PUT /sandboxes/{id}/files.
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
                &["file.written"],
            )
            .await
    }

    /// GET /sandboxes/{id}/artifacts (binary).
    pub async fn download_artifact(&self, box_id: &str, path: &str) -> Result<Vec<u8>, BoxError> {
        self.transport
            .request_binary(
                "GET",
                &format!("{}/artifacts", Self::box_path(box_id)?),
                &[("path", path.to_string())],
            )
            .await
    }

    /// GET /sandboxes/{id}/events, one page of the server's own size. An
    /// answer without `pageInfo` is the last page.
    pub async fn list_events(
        &self,
        box_id: &str,
        cursor: &str,
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
                    ("sort", sort.to_string()),
                    ("type", event_type.to_string()),
                ],
                &["events.list"],
            )
            .await?;
        let rows = match value.get("events") {
            Some(Value::Array(rows)) => rows,
            Some(other) => {
                return Err(BoxError::transport(format!(
                    "Boat answered the events of {box_id} with events that are not a list: {other}"
                )))
            }
            None => {
                return Err(BoxError::transport(format!(
                    "Boat answered the events of {box_id} without events"
                )))
            }
        };
        let mut events = Vec::with_capacity(rows.len());
        for event in rows {
            let Value::Object(event) = event else {
                return Err(BoxError::transport(format!(
                    "Boat answered the events of {box_id} with an event that is not an object: {event}"
                )));
            };
            events.push(event.clone());
        }
        let (next_cursor, has_more) = match value.get("pageInfo").and_then(Value::as_object) {
            Some(page) => (jstr(page.get("nextCursor")), jbool(page.get("hasMore"))),
            None => (String::new(), false),
        };
        Ok(BoxEventPage {
            events,
            next_cursor,
            has_more,
        })
    }
}
