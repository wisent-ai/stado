//! A backup restored into another Postgres server and proved there: the
//! roles the source used are made first (its policies and grants name them),
//! the archive is restored without owners or grants (the target's users are
//! its own) and creates each extension in the schema the source kept it in,
//! and every table the manifest counted is counted again on the target. The
//! comparison, not `pg_restore`'s exit status, decides: a hosted provider's
//! archive names objects and extensions a plain server lacks, and those
//! errors are reported beside a result whose rows are all there.

use serde_json::{json, Value};
use std::process::Command;

use crate::cli::CmdError;
use crate::primitives::failure::FailureCode;

use super::tools::{program, query, server_major, Connection};
use super::{files, lines, row_counts, Backup};

/// `name` as one quoted SQL identifier.
pub(in crate::cli::database) fn identifier(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

/// The manifest's list `key` as strings, refused when it is not one.
fn listed(manifest: &Value, key: &str) -> Result<Vec<String>, CmdError> {
    manifest[key]
        .as_array()
        .ok_or_else(|| {
            CmdError::click(format!("the backup manifest holds no {key} list"))
                .stating(FailureCode::Config)
        })?
        .iter()
        .map(|value| {
            value.as_str().map(str::to_string).ok_or_else(|| {
                CmdError::click(format!(
                    "the backup manifest's {key} holds {value}, not a name"
                ))
                .stating(FailureCode::Config)
            })
        })
        .collect()
}

/// Restore `backup` into `target` and compare every table's rows; the report
/// names each table whose count differs or is missing (`mismatches`), every
/// line `pg_restore` printed, the roles created and the source's extensions
/// the target does not have after the restore.
pub(super) fn restore(backup: &Backup, target: &Connection) -> Result<Value, CmdError> {
    let source_major = backup.manifest["server_major"].as_str().ok_or_else(|| {
        CmdError::click("the backup manifest holds no server_major").stating(FailureCode::Config)
    })?;
    let ca_file = files::server_ca(&backup.directory, "target", target)?;
    let ca = ca_file.as_deref();
    let target_major = server_major(target, ca)?;
    if target_major != source_major {
        return Err(CmdError::click(format!(
            "{} runs PostgreSQL {target_major} and the backup is of PostgreSQL {source_major}: the target must run the source's version",
            target.address()
        ))
        .stating(FailureCode::Config));
    }
    let psql = program("psql", Some(source_major))?;
    let pg_restore = program("pg_restore", Some(source_major))?;

    let present = lines(&query(
        &psql,
        target,
        ca,
        "select rolname from pg_roles",
        "listing the target's roles",
    )?);
    let mut roles_created = Vec::new();
    for role in listed(&backup.manifest, "roles")? {
        if present.contains(&role) {
            continue;
        }
        query(
            &psql,
            target,
            ca,
            &format!("create role {} nologin", identifier(&role)),
            &format!("creating role {role}"),
        )?;
        roles_created.push(role);
    }
    let mut command = Command::new(&pg_restore);
    command.args(["--no-owner", "--no-privileges", "--no-password"]);
    target.apply(&mut command, "--dbname", ca);
    command.arg(backup.archive());
    let restored = crate::wait::output(&mut command).map_err(|error| {
        CmdError::click(format!(
            "pg_restore into {} could not start: {error}",
            target.address()
        ))
        .stating(crate::cli::entry::error::io_failure_code(error.kind()))
    })?;
    let restore_errors: Vec<String> = String::from_utf8_lossy(&restored.stderr)
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect();

    let installed = lines(&query(
        &psql,
        target,
        ca,
        "select extname from pg_extension",
        "listing the target's extensions",
    )?);
    let extensions_unavailable: Vec<String> = listed(&backup.manifest, "extensions")?
        .into_iter()
        .filter(|extension| !installed.contains(extension))
        .collect();
    let counted = row_counts(&psql, target, ca)?;
    let source = backup.manifest["tables"].as_object().ok_or_else(|| {
        CmdError::click("the backup manifest holds no tables object").stating(FailureCode::Config)
    })?;
    let mut mismatches = Vec::new();
    for (table, rows) in source {
        let found = counted.tables.get(table);
        if found != Some(rows) {
            mismatches.push(json!({ "table": table, "source_rows": rows, "target_rows": found }));
        }
    }
    Ok(json!({
        "target": target.address(),
        "pg_restore_status": restored.status.to_string(),
        "pg_restore_errors": restore_errors,
        "roles_created": roles_created,
        "extensions_unavailable": extensions_unavailable,
        "tables": counted.tables.len(),
        "rows": counted.rows,
        "mismatches": mismatches,
    }))
}

/// Whether `report` from [`restore`] proves every counted table arrived.
pub(super) fn complete(report: &Value) -> bool {
    report["mismatches"].as_array().is_some_and(Vec::is_empty)
}
