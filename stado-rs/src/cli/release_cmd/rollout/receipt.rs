//! The delivery receipt a host keeps beside a delivered release.

use std::path::Path;

use crate::cli::CmdError;

/// The receipt at `path`, or none when this version was never delivered
/// here. Only a missing receipt is none: one that cannot be read or parsed is
/// refused with its path and cause, since read as none it skipped the check of
/// the receipt against the signed release.
pub(super) fn read(path: &Path) -> Result<Option<serde_json::Value>, CmdError> {
    match std::fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes).map(Some).map_err(|error| {
            CmdError::refused(format!(
                "the delivery receipt {} is not JSON: {error}",
                path.display()
            ))
        }),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(CmdError::click(format!(
            "the delivery receipt {} cannot be read: {error}",
            path.display()
        ))
        .stating(crate::cli::entry::error::io_failure_code(error.kind()))),
    }
}
