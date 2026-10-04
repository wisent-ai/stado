//! The janitor's owner-controlled state file and the records it carries.

pub(crate) mod error;
pub(crate) mod report;
pub(crate) mod write;

use std::fs::OpenOptions;
use std::io::{self, Read};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::Path;

use serde_json::{Map, Value};

use crate::providers::local::disk_cleanup::janitor::pass::lock::euid;
use crate::providers::local::disk_cleanup::janitor::state::error::JanitorError;
use crate::providers::local::disk_cleanup::janitor::STATE_NAME;

// ---------------------------------------------------------------------------
// state file read / write
// ---------------------------------------------------------------------------

/// Python `_read_state`: owner-controlled, no-follow, plain-dict JSON.
pub(crate) fn read_state(state_dir: &Path) -> Result<Value, JanitorError> {
    let path = state_dir.join(STATE_NAME);
    let file = match OpenOptions::new()
        .read(true)
        .custom_flags(nix::libc::O_NOFOLLOW)
        .open(&path)
    {
        Ok(file) => file,
        Err(exc) if exc.kind() == io::ErrorKind::NotFound => return Ok(Value::Object(Map::new())),
        Err(exc) => return Err(exc.into()),
    };
    let info = file.metadata()?;
    if !info.is_file() || info.uid() != euid() {
        return Err(JanitorError::os("unsafe cleanup state"));
    }
    let mut text = String::new();
    (&file).read_to_string(&mut text)?;
    let value: Value = serde_json::from_str(&text)?;
    Ok(if value.is_object() {
        value
    } else {
        Value::Object(Map::new())
    })
}
