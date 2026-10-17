//! Boat file, artifact, event and named-snapshot payload readers.

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

    /// POST /named-snapshots: freeze `box_id`'s disk under `name`. Boat
    /// answers at once with the snapshot `saving`; its status turns `ready`
    /// or `failed` in the background.
    pub async fn save_named_snapshot(
        &self,
        box_id: &str,
        name: &str,
    ) -> Result<Map<String, Value>, BoxError> {
        let body = json!({"sandboxId": Self::validate_box_id(box_id)?, "name": name});
        self.transport
            .request_json(
                "POST",
                "/named-snapshots",
                Some(&body),
                &[],
                &["snapshot.named.saving"],
            )
            .await
    }

    /// GET /named-snapshots, newest first, with the account's allowance.
    pub async fn named_snapshots(&self) -> Result<Map<String, Value>, BoxError> {
        self.transport
            .request_json(
                "GET",
                "/named-snapshots",
                None,
                &[],
                &["snapshot.named.list"],
            )
            .await
    }

    /// GET /named-snapshots/{name}.
    pub async fn named_snapshot(&self, name: &str) -> Result<Map<String, Value>, BoxError> {
        self.transport
            .request_json(
                "GET",
                &named_snapshot_path(name)?,
                None,
                &[],
                &["snapshot.named.info"],
            )
            .await
    }

    /// DELETE /named-snapshots/{name}: remove it for good.
    pub async fn delete_named_snapshot(&self, name: &str) -> Result<Map<String, Value>, BoxError> {
        self.transport
            .request_json(
                "DELETE",
                &named_snapshot_path(name)?,
                None,
                &[],
                &["snapshot.named.deleted"],
            )
            .await
    }

    /// GET /named-snapshots/{name}/shares: its share codes, active and
    /// revoked, without the codes themselves.
    pub async fn snapshot_shares(&self, name: &str) -> Result<Map<String, Value>, BoxError> {
        let path = format!("{}/shares", named_snapshot_path(name)?);
        let accepted = ["snapshot.share.list"];
        self.transport
            .request_json("GET", &path, None, &[], &accepted)
            .await
    }

    /// POST /named-snapshots/{name}/shares: a secret code with which any
    /// Boat account creates a sandbox from this snapshot (`create --from`).
    /// The answer carries `shareCode`, which Boat never shows again.
    pub async fn share_snapshot(&self, name: &str) -> Result<Map<String, Value>, BoxError> {
        let path = format!("{}/shares", named_snapshot_path(name)?);
        let accepted = ["snapshot.share.created"];
        self.transport
            .request_json("POST", &path, None, &[], &accepted)
            .await
    }

    /// DELETE /named-snapshots/{name}/shares/{share}: the code stops creating
    /// sandboxes; those already created from it keep their copy.
    pub async fn revoke_snapshot_share(
        &self,
        name: &str,
        share: &str,
    ) -> Result<Map<String, Value>, BoxError> {
        let shares = format!("{}/shares", named_snapshot_path(name)?);
        let path = segment_path(&shares, share, "a share id")?;
        let accepted = ["snapshot.share.revoked"];
        self.transport
            .request_json("DELETE", &path, None, &[], &accepted)
            .await
    }

    /// GET /sandboxes/{id}/snapshots: every snapshot of a sandbox's disk.
    pub async fn sandbox_snapshots(&self, box_id: &str) -> Result<Map<String, Value>, BoxError> {
        self.transport
            .request_json(
                "GET",
                &format!("{}/snapshots", Self::box_path(box_id)?),
                None,
                &[],
                &["snapshot.list"],
            )
            .await
    }

    /// GET /sandboxes/{id}/snapshots/latest: the snapshot resume and fork
    /// start from, null when the sandbox has none.
    pub async fn latest_snapshot(&self, box_id: &str) -> Result<Map<String, Value>, BoxError> {
        self.transport
            .request_json(
                "GET",
                &format!("{}/snapshots/latest", Self::box_path(box_id)?),
                None,
                &[],
                &["snapshot.latest"],
            )
            .await
    }

    /// GET /snapshots/{snapshot}/tree: the files and folders a snapshot
    /// holds, with their sizes, as a flat list.
    pub async fn snapshot_tree(&self, snapshot: &str) -> Result<Map<String, Value>, BoxError> {
        self.transport
            .request_json(
                "GET",
                &format!("{}/tree", snapshot_path(snapshot)?),
                None,
                &[],
                &["snapshot.tree"],
            )
            .await
    }

    /// GET /snapshots/{snapshot}/files: a file's bytes, or a folder as a
    /// tar, from a snapshot; the sandbox need not run.
    pub async fn snapshot_file(&self, snapshot: &str, path: &str) -> Result<Vec<u8>, BoxError> {
        self.transport
            .request_binary(
                "GET",
                &format!("{}/files", snapshot_path(snapshot)?),
                &[("path", path.to_string())],
            )
            .await
    }
}

/// One path segment for `value` under `collection`, encoded so a slash or a
/// query mark in it cannot address another resource; an empty value names
/// nothing and is refused naming `what`.
pub(super) fn segment_path(collection: &str, value: &str, what: &str) -> Result<String, BoxError> {
    if value.is_empty() {
        return Err(BoxError::value(format!("{what} is required")));
    }
    let segment: String = url::form_urlencoded::byte_serialize(value.as_bytes()).collect();
    Ok(format!("{collection}/{segment}"))
}

/// The path of one named snapshot; which names Boat accepts is Boat's to say.
fn named_snapshot_path(name: &str) -> Result<String, BoxError> {
    segment_path("/named-snapshots", name, "a named snapshot's name")
}

/// The path of one snapshot by the id Boat answered for it.
fn snapshot_path(snapshot: &str) -> Result<String, BoxError> {
    segment_path("/snapshots", snapshot, "a snapshot id")
}
