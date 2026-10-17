//! Parsing of a Boat sandbox record into a `BoxInfo`.
//!
//! The one reader that rejects a response whose id does not conform to the
//! sandbox-id pattern.

use serde_json::{Map, Value};

use super::super::constants::box_id_pattern;
use super::super::errors::BoxError;
use super::super::records::BoxInfo;
use super::coercion::{jbool, jstr};

/// The sandbox an answer carries under its `sandbox` key, as create, get,
/// update, fork, stop and resume answer.
pub fn parse_sandbox_envelope(payload: &Map<String, Value>) -> Result<BoxInfo, BoxError> {
    match payload.get("sandbox") {
        Some(Value::Object(record)) => parse_box_info(record),
        Some(other) => Err(BoxError::transport(format!(
            "Boat answered {} with a sandbox that is not an object: {other}",
            jstr(payload.get("type"))
        ))),
        None => Err(BoxError::transport(format!(
            "Boat answered {} without the sandbox it acted on",
            jstr(payload.get("type"))
        ))),
    }
}

/// One sandbox record: a pattern-conforming id is required, every other
/// field is read as Boat sent it.
pub fn parse_box_info(record: &Map<String, Value>) -> Result<BoxInfo, BoxError> {
    let box_id = jstr(record.get("id"));
    if !box_id_pattern().is_match(&box_id) {
        return Err(BoxError::transport(format!(
            "Boat answered a sandbox whose id {box_id:?} is not a Boat sandbox id"
        )));
    }
    Ok(BoxInfo {
        box_id,
        name: jstr(record.get("name")),
        state: jstr(record.get("state")),
        ip: jstr(record.get("ip")),
        url: jstr(record.get("url")),
        subdomain: jstr(record.get("subdomain")),
        created_at: jstr(record.get("createdAt")),
        updated_at: jstr(record.get("updatedAt")),
        archive_after: jstr(record.get("archiveAfter")),
        snapshot_available: jbool(record.get("snapshotAvailable")),
        snapshot_completed_at: jstr(record.get("snapshotCompletedAt")),
        last_snapshot_attempt_at: jstr(record.get("lastSnapshotAttemptAt")),
        last_snapshot_status: jstr(record.get("lastSnapshotStatus")),
    })
}
