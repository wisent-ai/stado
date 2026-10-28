//! The files a backup keeps: its owner-only directory, the archive it
//! restricts to its owner, the server's certificate authority libpq reads,
//! and the archive's size and SHA-256.

use serde_json::Value;
use std::path::{Path, PathBuf};

use crate::cli::CmdError;
use crate::primitives::failure::FailureCode;

use super::tools::Connection;

/// Where every backup is kept, below the home of the machine that took it.
const BACKUP_ROOT: &str = ".stado/database-backups";
const SERVER_CA: &str = "server-ca.pem";

/// `stado database backups [NAME] [--json]`: every backup kept on this
/// machine, newest first, with its time, tables, rows, size and SHA-256.
#[derive(Debug, clap::Args)]
pub(crate) struct BackupsArgs {
    /// Declared database whose backups are listed; every declared database when omitted.
    pub(in crate::cli::database) name: Option<String>,
    /// Emit machine-readable output.
    #[arg(long)]
    pub(in crate::cli::database) json: bool,
}

/// `~/.stado/database-backups`, below the home of the machine that runs
/// ([`crate::config_file::home`]: HOME, else the user database).
fn root() -> Result<PathBuf, CmdError> {
    crate::config_file::home()
        .map(|home| home.join(BACKUP_ROOT))
        .map_err(|cause| {
            CmdError::click(format!("database backups have no root: {cause}"))
                .stating(FailureCode::Config)
        })
}

/// `~/.stado/database-backups/<name>/<stamp>/`, created owner-only; a
/// directory of that stamp already there is refused, never written over.
pub(super) fn backup_directory(name: &str, stamp: &str) -> Result<PathBuf, CmdError> {
    let parent = root()?.join(name);
    let directory = parent.join(stamp);
    use std::os::unix::fs::DirBuilderExt;
    let created = |path: &Path, recursive: bool| {
        std::fs::DirBuilder::new()
            .recursive(recursive)
            .mode(crate::primitives::file_mode::widen(nix::libc::S_IRWXU))
            .create(path)
            .map_err(|error| {
                CmdError::click(format!("create {}: {error}", path.display()))
                    .stating(crate::cli::entry::error::io_failure_code(error.kind()))
            })
    };
    created(&parent, true)?;
    created(&directory, false)?;
    Ok(directory)
}

/// A kept backup directory with its manifest, or the cause it has none.
type Kept = (PathBuf, Result<Value, String>);

/// Every backup directory kept for `name`, newest first (the stamps sort by
/// time), each with its manifest, or with the cause it has none: a backup
/// that stopped before its manifest was written is incomplete, and named so.
fn kept(name: &str) -> Result<Vec<Kept>, CmdError> {
    let parent = root()?.join(name);
    let entries = match std::fs::read_dir(&parent) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => {
            return Err(CmdError::from(error).within(format!("read {}", parent.display())))
        }
    };
    let mut directories = Vec::new();
    for entry in entries {
        let path = entry
            .map_err(|error| CmdError::from(error).within(format!("read {}", parent.display())))?
            .path();
        if crate::primitives::presence::present(&path)? && path.is_dir() {
            directories.push(path);
        }
    }
    directories.sort();
    directories.reverse();
    Ok(directories
        .into_iter()
        .map(|directory| {
            let manifest = directory.join(super::MANIFEST);
            let read = std::fs::read_to_string(&manifest)
                .map_err(|error| format!("{} cannot be read: {error}", manifest.display()))
                .and_then(|text| {
                    serde_json::from_str(&text)
                        .map_err(|error| format!("{} is not JSON: {error}", manifest.display()))
                });
            (directory, read)
        })
        .collect())
}

/// Remove the complete backups of `name` beyond the newest `keep`, after a
/// backup succeeded; incomplete ones are left for the operator to read.
/// Returns the directories removed.
pub(super) fn prune(name: &str, keep: std::num::NonZeroUsize) -> Result<Vec<String>, CmdError> {
    let mut removed = Vec::new();
    for (directory, _) in kept(name)?
        .into_iter()
        .filter(|(_, manifest)| manifest.is_ok())
        .skip(keep.get())
    {
        std::fs::remove_dir_all(&directory).map_err(|error| {
            CmdError::from(error).within(format!("remove the older backup {}", directory.display()))
        })?;
        removed.push(directory.display().to_string());
    }
    Ok(removed)
}

/// `stado database backups [NAME] [--json]`: every kept backup of one
/// declared database, or of every declared database, newest first, with
/// when it was taken, its tables, rows, size and SHA-256.
pub(in crate::cli::database) fn list(
    name: Option<&str>,
    json_output: bool,
) -> Result<(), CmdError> {
    let declared = super::super::declared_databases()?;
    let names: Vec<&str> = match name {
        Some(name) if declared.contains_key(name) => vec![name],
        Some(name) => {
            return Err(CmdError::usage(format!(
                "database {name:?} is not declared"
            )))
        }
        None => declared.keys().map(String::as_str).collect(),
    };
    let mut rows = Vec::new();
    for name in names {
        for (directory, manifest) in kept(name)? {
            rows.push(match manifest {
                Ok(manifest) => serde_json::json!({
                    "database": name,
                    "backup": directory.display().to_string(),
                    "created_at": manifest["created_at"],
                    "source": manifest["source"],
                    "tables": manifest["tables"].as_object().map(serde_json::Map::len),
                    "rows": manifest["rows"],
                    "bytes": manifest["bytes"],
                    "sha256": manifest["sha256"],
                }),
                Err(cause) => serde_json::json!({
                    "database": name,
                    "backup": directory.display().to_string(),
                    "incomplete": cause,
                }),
            });
        }
    }
    if json_output {
        println!("{}", serde_json::to_string_pretty(&rows)?);
        return Ok(());
    }
    for row in &rows {
        crate::cli::print_answer(row, false)?;
    }
    Ok(())
}

pub(in crate::cli::database) fn restrict(path: &Path) -> Result<(), CmdError> {
    use std::os::unix::fs::PermissionsExt;
    let owner_only = crate::primitives::file_mode::widen(nix::libc::S_IRUSR | nix::libc::S_IWUSR);
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(owner_only))
        .map_err(|error| CmdError::from(error).within(format!("restrict {}", path.display())))
}

pub(in crate::cli::database) fn write(path: &Path, text: &str) -> Result<(), CmdError> {
    std::fs::write(path, text).map_err(|error| {
        CmdError::click(format!("write {}: {error}", path.display()))
            .stating(crate::cli::entry::error::io_failure_code(error.kind()))
    })
}

/// The item's certificate authority written into `directory` as
/// `prefix-server-ca.pem`, where libpq reads it; none when the item carries
/// none.
pub(in crate::cli::database) fn server_ca(
    directory: &Path,
    prefix: &str,
    connection: &Connection,
) -> Result<Option<PathBuf>, CmdError> {
    let Some(pem) = connection.ca_certificate() else {
        return Ok(None);
    };
    let file = directory.join(format!("{prefix}-{SERVER_CA}"));
    write(&file, pem)?;
    Ok(Some(file))
}

pub(super) fn digest(archive: &Path) -> Result<(u64, String), CmdError> {
    let digest = crate::primitives::digest::sha256_file(archive)
        .map_err(|error| CmdError::from(error).within("digest the dump archive".to_string()))?;
    Ok((digest.bytes, digest.sha256))
}
