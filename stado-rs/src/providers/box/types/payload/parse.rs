//! Parsing of the Box envelope into a `BoxInfo` record.
//!
//! Python `parse_box_info`, the one reader that rejects a response whose id
//! does not conform to the box-id pattern.

use serde_json::{Map, Value};

use super::super::constants::box_id_pattern;
use super::super::errors::BoxError;
use super::super::records::BoxInfo;
use super::coercion::{jbool, jstr, required_dict};

/// Python `parse_box_info`: unwrap the `"box"` envelope when present,
/// require a pattern-conforming id, default every other field.
pub fn parse_box_info(payload: &Map<String, Value>) -> Result<BoxInfo, BoxError> {
    let box_value = payload
        .get("box")
        .cloned()
        .unwrap_or(Value::Object(payload.clone()));
    let boxed = required_dict(box_value, "box")?;
    let box_id = jstr(boxed.get("id"));
    if !box_id_pattern().is_match(&box_id) {
        return Err(BoxError::transport(
            "Box response contains an invalid box id",
        ));
    }
    Ok(BoxInfo {
        box_id,
        name: jstr(boxed.get("name")),
        state: jstr(boxed.get("state")),
        ip: jstr(boxed.get("ip")),
        url: jstr(boxed.get("url")),
        subdomain: jstr(boxed.get("subdomain")),
        created_at: jstr(boxed.get("createdAt")),
        updated_at: jstr(boxed.get("updatedAt")),
        archive_after: jstr(boxed.get("archiveAfter")),
        snapshot_available: jbool(boxed.get("snapshotAvailable")),
        snapshot_completed_at: jstr(boxed.get("snapshotCompletedAt")),
        last_snapshot_attempt_at: jstr(boxed.get("lastSnapshotAttemptAt")),
        last_snapshot_status: jstr(boxed.get("lastSnapshotStatus")),
    })
}
