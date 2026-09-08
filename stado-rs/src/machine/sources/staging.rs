//! Copying the caller's archive into an owned work file, and reading the
//! retained object back out of storage to prove what was stored.

use std::io::{Read, Write};
#[cfg(any(target_os = "macos", target_os = "linux"))]
use std::os::unix::fs::OpenOptionsExt;

use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::machine::MachineError;
use crate::queue::JobStorage;

use super::archive::validate_staged_source_archive;
use super::{create_owned_machine_file, OwnedMachineFile, MAX_SOURCE_ARCHIVE_BYTES};

pub(in crate::machine) fn stage_source_archive(
    value: Option<&Value>,
) -> Result<Option<(OwnedMachineFile, String, u64)>, MachineError> {
    fn invalid(msg: impl Into<String>) -> MachineError {
        MachineError::new("INVALID_SOURCE_ARCHIVE", msg)
    }
    let Some(value) = value else {
        return Ok(None);
    };
    if value.is_null() || value.as_str() == Some("") {
        return Ok(None);
    }
    let Some(raw) = value.as_str() else {
        return Err(invalid("source_archive_path must be a string"));
    };
    let path = crate::config_file::expand_tilde(raw);
    let metadata = std::fs::symlink_metadata(&path)
        .map_err(|error| invalid(format!("source archive is not readable: {error}")))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(invalid("source archive must be a regular non-symlink file"));
    }
    let mut open = std::fs::OpenOptions::new();
    open.read(true);
    #[cfg(target_os = "macos")]
    open.custom_flags(0x0000_0100);
    #[cfg(target_os = "linux")]
    open.custom_flags(0x0002_0000);
    let mut source = open
        .open(&path)
        .map_err(|error| invalid(format!("source archive is not readable: {error}")))?;
    let opened_metadata = source
        .metadata()
        .map_err(|error| invalid(format!("source archive is not readable: {error}")))?;
    if !opened_metadata.is_file() {
        return Err(invalid("source archive must be a regular non-symlink file"));
    }
    let (staged, mut output) = create_owned_machine_file("source")
        .map_err(|error| invalid(format!("cannot stage source archive: {error}")))?;
    let mut digest = Sha256::new();
    let mut total = 0u64;
    let mut chunk = [0u8; 1024 * 1024];
    loop {
        let read = source
            .read(&mut chunk)
            .map_err(|error| invalid(format!("source archive is not readable: {error}")))?;
        if read == 0 {
            break;
        }
        total = total
            .checked_add(read as u64)
            .ok_or_else(|| invalid("source archive size overflows"))?;
        if total > MAX_SOURCE_ARCHIVE_BYTES {
            return Err(invalid(format!(
                "source archive must be between 1 and {MAX_SOURCE_ARCHIVE_BYTES} bytes"
            )));
        }
        output
            .write_all(&chunk[..read])
            .map_err(|error| invalid(format!("cannot stage source archive: {error}")))?;
        digest.update(&chunk[..read]);
    }
    if total == 0 {
        return Err(invalid(format!(
            "source archive must be between 1 and {MAX_SOURCE_ARCHIVE_BYTES} bytes"
        )));
    }
    output
        .sync_all()
        .map_err(|error| invalid(format!("cannot sync staged source archive: {error}")))?;
    if let Some(parent) = staged.path().parent() {
        std::fs::File::open(parent)
            .and_then(|directory| directory.sync_all())
            .map_err(|error| invalid(format!("cannot sync source work root: {error}")))?;
    }
    drop(output);
    validate_staged_source_archive(staged.path())?;
    Ok(Some((staged, hex::encode(digest.finalize()), total)))
}
pub(in crate::machine) async fn readback_machine_source(
    store: &JobStorage,
    blob_path: &str,
) -> Result<Option<OwnedMachineFile>, MachineError> {
    let (readback, file) = create_owned_machine_file("readback")
        .map_err(|error| MachineError::retryable("SOURCE_UPLOAD_FAILED", error.to_string()))?;
    drop(file);
    if !store
        .download_blob(blob_path, readback.path())
        .await
        .map_err(|error| MachineError::retryable("SOURCE_UPLOAD_FAILED", error.to_string()))?
    {
        readback
            .cleanup()
            .map_err(|error| MachineError::retryable("SOURCE_UPLOAD_FAILED", error.to_string()))?;
        return Ok(None);
    }
    std::fs::File::open(readback.path())
        .and_then(|file| file.sync_all())
        .map_err(|error| MachineError::retryable("SOURCE_UPLOAD_FAILED", error.to_string()))?;
    Ok(Some(readback))
}
