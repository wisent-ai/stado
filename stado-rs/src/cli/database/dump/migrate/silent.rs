//! Whether a kept backup may be restored: only when the source no longer
//! answers. While it answers, a new backup (`migrate` without
//! `--from-backup`) holds every write the kept one lacks, and restoring the
//! kept one would drop them.

use serde_json::Value;

use crate::cli::CmdError;

use super::super::files;
use super::super::tools::Connection;

/// Refuse restoring `directory` for `name` while the source `item` names
/// still answers. The source is asked for its server version through the
/// item's own session; an item that names no session the source could be
/// reached on counts as silent.
pub(super) fn source_silent(
    name: &str,
    item: &str,
    fields: &Value,
    directory: &std::path::Path,
) -> Result<(), CmdError> {
    let Ok(connection) = Connection::from_item(item, fields) else {
        return Ok(());
    };
    let ca_file = match connection.ca_certificate() {
        Some(pem) => {
            let file = directory.join("source-probe-server-ca.pem");
            files::write(&file, pem)?;
            Some(file)
        }
        None => None,
    };
    let answered = super::super::tools::server_major(&connection, ca_file.as_deref());
    if let Some(file) = &ca_file {
        let _ = std::fs::remove_file(file);
    }
    match answered {
        Ok(major) => Err(CmdError::usage(format!(
            "{name}'s source {} still answers (PostgreSQL {major}), so a new backup holds every write the kept {} lacks: run `stado database migrate {name} --to fleet` without --from-backup",
            connection.address(),
            directory.display()
        ))),
        Err(_) => Ok(()),
    }
}
