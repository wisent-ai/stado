//! Parsing of a Boat sandbox record into a `BoxInfo`.
//!
//! The one reader that rejects a response whose id does not conform to the
//! sandbox-id pattern.

use serde_json::{Map, Value};

use super::super::constants::box_id_pattern;
use super::super::errors::BoxError;
use super::super::records::BoxInfo;
use super::coercion::{jbool, jstr, shown};

/// The sandbox an answer carries under its `sandbox` key, as create, get,
/// update, fork, stop and resume answer.
pub fn parse_sandbox_envelope(payload: &Map<String, Value>) -> Result<BoxInfo, BoxError> {
    match payload.get("sandbox") {
        Some(Value::Object(record)) => parse_box_info(record),
        Some(other) => Err(BoxError::transport(format!(
            "Boat answered {} with a sandbox that is not an object: {other}",
            shown(payload.get("type"))
        ))),
        None => Err(BoxError::transport(format!(
            "Boat answered {} without the sandbox it acted on",
            shown(payload.get("type"))
        ))),
    }
}

/// One sandbox record: a pattern-conforming id is required, every other
/// field is read as Boat sent it.
pub fn parse_box_info(record: &Map<String, Value>) -> Result<BoxInfo, BoxError> {
    let text = |field: &str| jstr(record.get(field), field, "sandbox");
    let box_id = text("id")?;
    if !box_id_pattern().is_match(&box_id) {
        return Err(BoxError::transport(format!(
            "Boat answered a sandbox whose id {box_id:?} is not a Boat sandbox id"
        )));
    }
    Ok(BoxInfo {
        box_id,
        name: text("name")?,
        state: text("state")?,
        ip: text("ip")?,
        url: text("url")?,
        subdomain: text("subdomain")?,
        created_at: text("createdAt")?,
        updated_at: text("updatedAt")?,
        archive_after: text("archiveAfter")?,
        snapshot_available: jbool(
            record.get("snapshotAvailable"),
            "snapshotAvailable",
            "sandbox",
        )?,
        snapshot_completed_at: text("snapshotCompletedAt")?,
        last_snapshot_attempt_at: text("lastSnapshotAttemptAt")?,
        last_snapshot_status: text("lastSnapshotStatus")?,
    })
}
