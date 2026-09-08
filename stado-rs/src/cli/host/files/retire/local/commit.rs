use std::ffi::OsStr;
use std::fs::{File, Metadata};
use std::os::fd::AsRawFd;
use std::os::unix::fs::MetadataExt;
use std::path::Path;

use crate::cli::CmdError;

use crate::cli::host::files::retire::fs::dirs::{
    entry_exists_at, mkdir_at, open_directory_at, open_source_at, remove_empty_directory_at,
};
use crate::cli::host::files::retire::fs::rename::{rename_noreplace, rollback_retirement};
use crate::cli::host::files::retire::fs::{hash_open_file, source_unchanged};
use crate::cli::host::files::retire::{retire_refused, RetireFileOutcome};

/// The mutating half of [`super::retire_file_local_document`]: create the
/// transaction directory, prove the source has not moved, rename it
/// no-replace, and verify the destination or roll back.
#[allow(clippy::too_many_arguments)]
pub(super) fn commit_retirement(
    path: &str,
    source_metadata: &Metadata,
    destination_directories: &[File],
    source_directory: &File,
    source_name: &OsStr,
    transaction: String,
    destination: &Path,
    size: u64,
    sha256: String,
    mode: String,
    uid: u32,
) -> Result<RetireFileOutcome, CmdError> {
    let backups_directory = destination_directories
        .last()
        .ok_or_else(|| retire_refused("backup tree was not created"))?;
    if source_metadata.dev().ne(&backups_directory
        .metadata()
        .map_err(|error| retire_refused(format!("cannot inspect backup root: {error}")))?
        .dev())
    {
        return Err(retire_refused(
            "source and backup tree are not on one filesystem, so an atomic move is impossible",
        ));
    }
    if entry_exists_at(backups_directory.as_raw_fd(), OsStr::new(&transaction))? {
        return Err(retire_refused("destination transaction collision"));
    }
    mkdir_at(backups_directory.as_raw_fd(), OsStr::new(&transaction))?;
    let transaction_directory =
        open_directory_at(backups_directory.as_raw_fd(), OsStr::new(&transaction), uid)?
            .ok_or_else(|| retire_refused("transaction directory disappeared after creation"))?;
    let transaction_metadata = transaction_directory.metadata().map_err(|error| {
        retire_refused(format!("cannot inspect transaction directory: {error}"))
    })?;
    if transaction_metadata.mode() & 0o077 != 0 {
        remove_empty_directory_at(backups_directory.as_raw_fd(), OsStr::new(&transaction));
        return Err(retire_refused("transaction directory is not owner-only"));
    }
    if entry_exists_at(transaction_directory.as_raw_fd(), source_name)? {
        remove_empty_directory_at(backups_directory.as_raw_fd(), OsStr::new(&transaction));
        return Err(retire_refused("destination collision"));
    }

    let current_source = open_source_at(source_directory.as_raw_fd(), source_name)?
        .ok_or_else(|| retire_refused("source disappeared before the move"))?;
    let current_metadata = current_source
        .metadata()
        .map_err(|error| retire_refused(format!("cannot re-inspect source: {error}")))?;
    if !source_unchanged(source_metadata, &current_metadata) {
        remove_empty_directory_at(backups_directory.as_raw_fd(), OsStr::new(&transaction));
        return Err(retire_refused("source changed after it was observed"));
    }

    if let Err(error) = rename_noreplace(
        source_directory.as_raw_fd(),
        source_name,
        transaction_directory.as_raw_fd(),
        source_name,
    ) {
        remove_empty_directory_at(backups_directory.as_raw_fd(), OsStr::new(&transaction));
        return Err(retire_refused(format!(
            "atomic no-replace rename failed: {error}"
        )));
    }

    let postcondition = (|| -> Result<(), CmdError> {
        if entry_exists_at(source_directory.as_raw_fd(), source_name)? {
            return Err(retire_refused("source path was recreated during the move"));
        }
        let mut destination_file = open_source_at(transaction_directory.as_raw_fd(), source_name)?
            .ok_or_else(|| retire_refused("destination is absent after rename"))?;
        let destination_metadata = destination_file
            .metadata()
            .map_err(|error| retire_refused(format!("cannot inspect destination: {error}")))?;
        if !source_unchanged(source_metadata, &destination_metadata) {
            return Err(retire_refused(
                "destination inode, owner, size, or mode differs from the opened source",
            ));
        }
        if hash_open_file(&mut destination_file)? != sha256 {
            return Err(retire_refused(
                "destination SHA-256 differs from the opened source",
            ));
        }
        Ok(())
    })();
    if let Err(error) = postcondition {
        let rollback = rollback_retirement(
            source_directory.as_raw_fd(),
            source_name,
            transaction_directory.as_raw_fd(),
            source_name,
        );
        return Err(CmdError::click(format!("{error}; {rollback}")));
    }

    Ok(RetireFileOutcome {
        target: String::new(),
        source: path.to_string(),
        destination: Some(destination.to_string_lossy().into_owned()),
        transaction: Some(transaction),
        status: "retired".to_string(),
        size: Some(size),
        sha256: Some(sha256),
        mode: Some(mode),
        detail: None,
    })
}
