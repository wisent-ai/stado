//! The append-only half of the archive: one immutable event per transition,
//! the artifacts a phase publishes beside them, and the local mirror every
//! archived document is duplicated into by a durable, atomic write.

use std::fs;
use std::io::Write;
use std::path::Path;

use serde::Serialize;
use serde_json::Value;
use tempfile::NamedTempFile;
use uuid::Uuid;

use crate::cli::resources::journal::clock::{compact_now, now};
use crate::cli::resources::journal::names::{
    remote_path, validate_artifact_name, validate_operation_id,
};
use crate::cli::resources::journal::records::OperationEvent;
use crate::cli::resources::model::{canonical_json_bytes, SCHEMA_VERSION};
use crate::cli::CmdError;

use super::Journal;

impl Journal {
    pub async fn event(
        &self,
        operation_id: &str,
        event: &str,
        action_id: Option<&str>,
        detail: Value,
    ) -> Result<(), CmdError> {
        let record = OperationEvent {
            schema_version: SCHEMA_VERSION,
            event_id: Uuid::new_v4().to_string(),
            operation_id: operation_id.to_string(),
            recorded_at: now(),
            event: event.to_string(),
            action_id: action_id.map(str::to_string),
            detail,
        };
        let bytes = canonical_json_bytes(&record)?;
        let body =
            String::from_utf8(bytes.clone()).map_err(|error| CmdError::click(error.to_string()))?;
        let name = format!("events/{}-{}.json", compact_now(), record.event_id);
        let path = remote_path(operation_id, &name);
        if !self.store.create_text_if_absent(&path, &body).await? {
            return Err(CmdError::click("operation event id collision"));
        }
        self.write_local(operation_id, &name, &bytes)?;
        Ok(())
    }

    pub async fn write_artifact<T: Serialize>(
        &self,
        operation_id: &str,
        name: &str,
        value: &T,
    ) -> Result<(), CmdError> {
        validate_artifact_name(name)?;
        let bytes = canonical_json_bytes(value)?;
        let body =
            String::from_utf8(bytes.clone()).map_err(|error| CmdError::click(error.to_string()))?;
        let path = remote_path(operation_id, name);
        self.store.upload_text(&path, &body).await?;
        self.write_local(operation_id, name, &bytes)
    }

    pub(super) fn write_local(
        &self,
        operation_id: &str,
        name: &str,
        bytes: &[u8],
    ) -> Result<(), CmdError> {
        validate_operation_id(operation_id)?;
        validate_artifact_name(name)?;
        let path = self.local_root.join(operation_id).join(name);
        atomic_write(&path, bytes)
    }
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), CmdError> {
    let parent = path
        .parent()
        .ok_or_else(|| CmdError::click(format!("{} has no parent", path.display())))?;
    fs::create_dir_all(parent)?;

    let mut temporary = NamedTempFile::new_in(parent)?;
    temporary.write_all(bytes)?;
    temporary.as_file().sync_all()?;
    temporary.persist(path).map_err(|error| error.error)?;
    fs::File::open(parent)?.sync_all()?;
    Ok(())
}
