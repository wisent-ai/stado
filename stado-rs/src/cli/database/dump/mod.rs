//! `stado database backup` and the dump `stado database migrate` restores: a
//! declared Postgres database copied whole into storage the machine that
//! runs the command owns, whoever hosts the database.
//!
//! The credential item is read from the owner vault (from any host, through
//! the host channel), the server is reached on the item's `session_url` and
//! verified against its `ca_certificate`, and `pg_dump` of the server's own
//! major version writes one custom-format archive under
//! `~/.stado/database-backups/<name>/<UTC time>/`. Beside it,
//! `manifest.json` records what the archive must reproduce: every readable
//! table with its exact row count, the roles and extensions the database
//! uses, the schemas and tables the item's user may not read (left out and
//! named, never silently), the archive's size and SHA-256. A backup is kept
//! only after `pg_restore --list` reads the archive back.

use serde_json::{json, Map, Value};
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::cli::CmdError;
use crate::primitives::failure::FailureCode;

use super::supabase::owner_vault::{self, Owner};

pub(super) mod files;
pub(super) mod migrate;
pub(super) mod restore;
pub(super) mod tools;

use tools::{program, query, server_major, Connection};

const ARCHIVE: &str = "database.dump";
const MANIFEST: &str = "manifest.json";

/// Schemas Postgres keeps for itself; no product writes there.
const SYSTEM_SCHEMAS: &str = "nspname !~ '^pg_' and nspname <> 'information_schema'";

/// One backup: the directory that holds it and its manifest.
pub(super) struct Backup {
    pub(super) directory: PathBuf,
    pub(super) manifest: Value,
}

impl Backup {
    pub(super) fn archive(&self) -> PathBuf {
        self.directory.join(ARCHIVE)
    }
}

/// Every table the item's user may read with its exact row count, keyed by
/// its quoted `schema.table` name, and their sum.
pub(super) struct Counts {
    pub(super) tables: Map<String, Value>,
    pub(super) rows: u64,
}

/// `stado database backup [NAME] [--json]`.
///
/// Reads each database's credential item from the owner vault, reaches the
/// server on the item's `session_url` (verified against its
/// `ca_certificate`) and writes, with `pg_dump` of the server's own major
/// version, one custom-format archive under
/// `~/.stado/database-backups/<name>/<UTC time>/` (owner-only) with
/// `manifest.json`: every readable table and its exact row count, the roles
/// and extensions in use, the schemas and tables the item's user may not
/// read (left out and named), the archive's size and SHA-256. Without NAME,
/// every declared postgres database is backed up and each one that could not
/// be is named; the command then exits non-zero.
#[derive(Debug, clap::Args)]
pub(crate) struct BackupArgs {
    /// Declared database to back up; every declared postgres database when omitted.
    pub(super) name: Option<String>,
    /// After this backup succeeds, keep only the newest N complete backups of
    /// each database it took, removing older ones; without it none is removed.
    #[arg(long)]
    pub(super) keep: Option<std::num::NonZeroUsize>,
    /// Emit machine-readable output.
    #[arg(long)]
    pub(super) json: bool,
}

pub(super) async fn backup(
    name: Option<&str>,
    keep: Option<std::num::NonZeroUsize>,
    json_output: bool,
) -> Result<(), CmdError> {
    let declared = super::declared_databases()?;
    let names: Vec<&str> = match name {
        Some(name) => {
            if !declared.contains_key(name) {
                return Err(CmdError::usage(format!(
                    "database {name:?} is not declared"
                )));
            }
            vec![name]
        }
        None => declared
            .iter()
            .filter(|(_, declaration)| postgres(declaration.engine()))
            .map(|(name, _)| name.as_str())
            .collect(),
    };
    let owner = owner_vault::locate().await?;
    let mut rows = Vec::new();
    let mut failed = Vec::new();
    for name in names {
        match take(name, &owner).await {
            Ok(backup) => {
                let mut row = json!({
                    "database": name,
                    "backup": backup.directory.display().to_string(),
                    "tables": backup.manifest["tables"],
                    "rows": backup.manifest["rows"],
                    "bytes": backup.manifest["bytes"],
                    "sha256": backup.manifest["sha256"],
                    "excluded_schemas": backup.manifest["excluded_schemas"],
                    "excluded_tables": backup.manifest["excluded_tables"],
                });
                if let Some(keep) = keep {
                    match files::prune(name, keep) {
                        Ok(removed) => row["removed_older"] = json!(removed),
                        Err(error) => {
                            row["prune_refused"] = json!(error.to_string());
                            failed.push(format!(
                                "{name}: the backup was kept, older ones were not removed: {error}"
                            ));
                        }
                    }
                }
                rows.push(row);
            }
            Err(error) => {
                rows.push(json!({ "database": name, "refused": error.to_string() }));
                failed.push(format!("{name}: {error}"));
            }
        }
    }
    if json_output {
        println!("{}", serde_json::to_string_pretty(&rows)?);
    } else {
        for row in &rows {
            crate::cli::print_answer(row, false)?;
        }
    }
    if failed.is_empty() {
        return Ok(());
    }
    Err(CmdError::click(format!(
        "{} of {} databases were not backed up: {}",
        failed.len(),
        rows.len(),
        failed.join("; ")
    ))
    .stating(FailureCode::InfraDown))
}

fn postgres(engine: &str) -> bool {
    crate::config::DatabaseEngine::parse(engine) == Some(crate::config::DatabaseEngine::Postgres)
}

/// The item name and fields of `name`'s credential item, read where the
/// owner vault is.
pub(in crate::cli::database) async fn item_fields(
    name: &str,
    owner: &Owner,
) -> Result<(String, Value), CmdError> {
    let declared = super::declared_databases()?;
    let declaration = declared
        .get(name)
        .ok_or_else(|| CmdError::usage(format!("database {name:?} is not declared")))?;
    if !postgres(declaration.engine()) {
        return Err(CmdError::usage(format!(
            "database {name} is declared {}; backup and migrate copy a postgres database",
            declaration.engine()
        )));
    }
    let item = declaration.item().to_string();
    let document = owner.document(&item).await?.ok_or_else(|| {
        CmdError::click(format!(
            "{item} is not in {}'s owner vault, so {name} cannot be reached",
            owner.name()
        ))
        .stating(FailureCode::NotFound)
    })?;
    let fields = document
        .get("fields")
        .filter(|fields| fields.is_object())
        .cloned()
        .ok_or_else(|| {
            CmdError::click(format!("{item} holds no fields object")).stating(FailureCode::Config)
        })?;
    Ok((item, fields))
}

/// Take one backup of `name` into a new directory and return it.
pub(super) async fn take(name: &str, owner: &Owner) -> Result<Backup, CmdError> {
    let (item, fields) = item_fields(name, owner).await?;
    let connection = Connection::from_item(&item, &fields)?;
    let created = chrono::Utc::now();
    let directory = files::backup_directory(name, &created.format("%Y%m%dT%H%M%SZ").to_string())?;
    let ca_file = files::server_ca(&directory, "source", &connection)?;
    let ca = ca_file.as_deref();
    let major = server_major(&connection, ca)?;
    let psql = program("psql", Some(major.as_str()))?;
    let pg_dump = program("pg_dump", Some(major.as_str()))?;
    let pg_restore = program("pg_restore", Some(major.as_str()))?;

    let excluded_schemas = lines(&query(
        &psql,
        &connection,
        ca,
        &format!("select quote_ident(nspname) from pg_namespace where {SYSTEM_SCHEMAS} and not has_schema_privilege(oid, 'USAGE') order by nspname"),
        "listing the schemas the item's user may not read",
    )?);
    let excluded_tables = lines(&query(
        &psql,
        &connection,
        ca,
        &format!(
            "select format('%I.%I', n.nspname, c.relname) from pg_class c join pg_namespace n on n.oid = c.relnamespace \
             where c.relkind in ('r', 'p', 'm', 'S') and {} and has_schema_privilege(n.oid, 'USAGE') \
             and not has_table_privilege(c.oid, 'SELECT') order by n.nspname, c.relname",
            SYSTEM_SCHEMAS.replace("nspname", "n.nspname")
        ),
        "listing the tables the item's user may not read",
    )?);
    let roles = lines(&query(
        &psql,
        &connection,
        ca,
        "select rolname from pg_roles where rolname !~ '^pg_' order by rolname",
        "listing roles",
    )?);
    let extensions = lines(&query(
        &psql,
        &connection,
        ca,
        "select extname from pg_extension order by extname",
        "listing extensions",
    )?);
    let counts = row_counts(&psql, &connection, ca)?;

    let archive = directory.join(ARCHIVE);
    let mut dump = Command::new(&pg_dump);
    dump.arg("--format=custom")
        .arg(format!("--file={}", archive.display()))
        .arg("--no-password");
    for schema in &excluded_schemas {
        dump.arg(format!("--exclude-schema={schema}"));
    }
    for table in &excluded_tables {
        dump.arg(format!("--exclude-table={table}"));
    }
    connection.apply(&mut dump, "--dbname", ca);
    tools::output(
        &mut dump,
        &format!("pg_dump of {name} from {}", connection.address()),
    )?;
    files::restrict(&archive)?;
    let listed = tools::output(
        Command::new(&pg_restore).arg("--list").arg(&archive),
        &format!("pg_restore --list reading {} back", archive.display()),
    )?;
    if listed.trim().is_empty() {
        return Err(CmdError::click(format!(
            "{} reads back as an empty archive",
            archive.display()
        ))
        .stating(FailureCode::InfraDown));
    }
    let (bytes, sha256) = files::digest(&archive)?;
    let manifest = json!({
        "database": name,
        "item": item,
        "item_provider": fields.get("provider"),
        "project_ref": fields.get("project_ref"),
        "source": connection.address(),
        "server_major": major,
        "pg_dump": pg_dump.display().to_string(),
        "created_at": created.to_rfc3339(),
        "archive": archive.display().to_string(),
        "bytes": bytes,
        "sha256": sha256,
        "excluded_schemas": excluded_schemas,
        "excluded_tables": excluded_tables,
        "roles": roles,
        "extensions": extensions,
        "tables": counts.tables,
        "rows": counts.rows,
    });
    files::write(
        &directory.join(MANIFEST),
        &serde_json::to_string_pretty(&manifest)?,
    )?;
    Ok(Backup {
        directory,
        manifest,
    })
}

/// Count the rows of every table the item's user may read.
pub(super) fn row_counts(
    psql: &Path,
    connection: &Connection,
    ca: Option<&Path>,
) -> Result<Counts, CmdError> {
    let text = query(
        psql,
        connection,
        ca,
        "select format('%I.%I', table_schema, table_name), \
         (xpath('/row/c/text()', query_to_xml(format('select count(*) as c from %I.%I', table_schema, table_name), false, true, '')))[1]::text \
         from information_schema.tables where table_type = 'BASE TABLE' \
         and table_schema !~ '^pg_' and table_schema <> 'information_schema' \
         and has_schema_privilege(table_schema, 'USAGE') \
         and has_table_privilege(format('%I.%I', table_schema, table_name), 'SELECT') \
         order by table_schema, table_name",
        "counting the rows of every table",
    )?;
    let mut tables = Map::new();
    let mut each = Vec::new();
    for line in text.lines().filter(|line| !line.trim().is_empty()) {
        let (table, count) = line.split_once('\t').ok_or_else(|| {
            CmdError::click(format!(
                "{} answered the row count line {line:?} without a count",
                connection.address()
            ))
            .stating(FailureCode::InfraDown)
        })?;
        let count: u64 = count.trim().parse().map_err(|error| {
            CmdError::click(format!(
                "{} counted {count:?} rows in {table}, not a number: {error}",
                connection.address()
            ))
            .stating(FailureCode::InfraDown)
        })?;
        each.push(count);
        tables.insert(table.to_string(), json!(count));
    }
    Ok(Counts {
        tables,
        rows: each.iter().sum(),
    })
}

pub(super) fn lines(text: &str) -> Vec<String> {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect()
}
