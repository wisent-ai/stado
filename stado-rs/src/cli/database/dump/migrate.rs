//! `stado database migrate NAME --to fleet [--port PORT] [--host HOST]`: a
//! declared Postgres database another provider hosts (a Supabase project, an
//! external server) moved onto a fleet host Stado runs, with no vendor and
//! no bill.
//!
//! The order keeps every step recoverable. A backup is taken first and kept
//! (`stado database backup`). The credential item as it stood is kept as
//! `<item>-previous`. The fleet server is placed with the source's major
//! PostgreSQL version, which writes `<item>` with the fleet coordinates; the
//! backup is restored there and every counted table is counted again. When a
//! table differs, `<item>` gets the source's coordinates back, the fleet
//! coordinates are kept as `<item>-fleet-pending` so a second run restores
//! into the same server instead of placing another, and the command refuses
//! naming each table. The declaration (engine, consumers) never changes: a
//! consumer that opens the database through `stado_database::connect` reads
//! the new item on its next connection. The source is left as it was;
//! `stado database destroy` is a separate decision.
//!
//! Writes the source takes after the backup are not in the copy, and a
//! consumer that connects while the restore runs reaches the fleet server
//! before its rows are all there (a write it makes there makes a count
//! differ): stop the consumers and the source's writers first.

use serde_json::{json, Value};

use crate::cli::CmdError;
use crate::primitives::failure::FailureCode;

use super::super::supabase::owner_vault::{self, Owner};
use super::super::DatabaseProvider;
use super::tools::Connection;
use super::{files, item_fields, restore, take};

/// `stado database migrate NAME --to fleet [--port PORT] [--host HOST] [--json]`.
#[derive(Debug, clap::Args)]
pub(crate) struct MigrateArgs {
    /// Declared postgres database to move.
    pub(in crate::cli::database) name: String,
    /// Where the database goes: fleet (Stado, on a fleet host).
    #[arg(long, value_enum)]
    pub(in crate::cli::database) to: DatabaseProvider,
    /// Fleet host the database is placed on (default: the vault owner).
    #[arg(long)]
    pub(in crate::cli::database) host: Option<String>,
    /// Port the fleet database listens on; without it the fleet host hands
    /// out a free one and the placement reports it.
    #[arg(long)]
    pub(in crate::cli::database) port: Option<u16>,
    /// Restore this kept backup (a directory `stado database backups NAME`
    /// lists) instead of taking a new one: the move of a database whose
    /// provider paused or deleted it, so nothing can be read from the source.
    /// Its manifest must name NAME and its archive must still hash to the
    /// SHA-256 recorded when it was read back. Writes the source took after
    /// that backup are not in it, and the report says when it was taken.
    #[arg(long)]
    pub(in crate::cli::database) from_backup: Option<std::path::PathBuf>,
    /// Emit machine-readable output.
    #[arg(long)]
    pub(in crate::cli::database) json: bool,
}

/// `migrate` with its arguments as the command line gave them: only the
/// fleet provider is a destination Stado places.
pub(in crate::cli::database) async fn run(args: MigrateArgs) -> Result<(), CmdError> {
    if args.to != DatabaseProvider::Fleet {
        return Err(CmdError::usage(format!(
            "--to {} is not a destination migrate places: a database moves to fleet; \
             bring a database onto another provider with `stado database create --provider {}`",
            args.to, args.to
        )));
    }
    migrate(&args.name, args.host.as_deref(), args.port, args.from_backup.as_deref(), args.json).await
}

/// The provider an item records in `provider`, or none when it records none
/// (a Supabase item written before the field existed carries `project_ref`).
pub(in crate::cli::database) fn provider(
    item: &str,
    fields: &Value,
) -> Result<Option<DatabaseProvider>, CmdError> {
    match fields.get("provider") {
        None | Some(Value::Null) => Ok(None),
        Some(word) => serde_json::from_value(word.clone())
            .map(Some)
            .map_err(|error| {
                CmdError::click(format!(
                    "{item}.provider is {word}, not fleet, supabase or external: {error}"
                ))
                .stating(FailureCode::Config)
            }),
    }
}

/// The whole document of `item`, refused when the vault holds none.
async fn document(owner: &Owner, item: &str) -> Result<Value, CmdError> {
    owner.document(item).await?.ok_or_else(|| {
        CmdError::click(format!("{item} is not in {}'s owner vault", owner.name()))
            .stating(FailureCode::NotFound)
    })
}

async fn store(owner: &Owner, item: &str, document: &Value) -> Result<(), CmdError> {
    owner
        .store(item, "bundle", &document["fields"], &document["context"])
        .await
}

/// Keep `document`, a copy of `item` as it stands, under `kept`, with a
/// context that says what it is: the item it was copied from and when.
/// Skarbiec refuses a second item whose whole payload equals one it holds,
/// and every copy migrate keeps (the source's coordinates before the move,
/// the fleet ones when a move fails) is written while `item` still holds that
/// same payload, so a bare copy was refused and no database could be moved.
async fn keep_copy(owner: &Owner, kept: &str, item: &str, document: &Value) -> Result<(), CmdError> {
    let mut copy = document.clone();
    copy["context"]["kept_from"] = json!(item);
    copy["context"]["kept_at"] = json!(crate::models::isoformat_utc(chrono::Utc::now()));
    store(owner, kept, &copy).await
}

pub(in crate::cli::database) async fn migrate(
    name: &str,
    host: Option<&str>,
    port: Option<u16>,
    from_backup: Option<&std::path::Path>,
    json_output: bool,
) -> Result<(), CmdError> {
    let owner = owner_vault::locate().await?;
    let (item, fields) = item_fields(name, &owner).await?;
    if item != format!("{name}-database") {
        return Err(CmdError::usage(format!(
            "{name} is declared with item {item}; a fleet placement writes {name}-database, so migrate moves only a database whose item is that"
        )));
    }
    if provider(&item, &fields)? == Some(DatabaseProvider::Fleet) {
        return Err(CmdError::usage(format!(
            "{name} is already a fleet database ({item}.provider is fleet)"
        )));
    }
    let previous = format!("{item}-previous");
    let pending = format!("{item}-fleet-pending");
    let source_document = document(&owner, &item).await?;
    if !source_document["context"].is_object() {
        return Err(CmdError::click(format!(
            "{item} holds no context object, so its coordinates could not be written back if the move failed; \
             `stado database adopt {name}` rewrites a Supabase item whole"
        ))
        .stating(FailureCode::Config));
    }

    let backup = match from_backup {
        Some(directory) => {
            source_silent(name, &item, &fields, directory)?;
            files::kept_backup(name, directory)?
        }
        None => take(name, &owner).await?,
    };
    let major = backup.manifest["server_major"]
        .as_str()
        .ok_or_else(|| {
            CmdError::click("the backup manifest holds no server_major")
                .stating(FailureCode::Config)
        })?
        .to_string();
    keep_copy(&owner, &previous, &item, &source_document).await?;

    let pending_kept = match owner.item_ids().await? {
        Some(ids) if !ids.contains(&pending) => None,
        _ => owner.document(&pending).await?,
    };
    let from_pending = pending_kept.is_some();
    let (placed_on, placement, target_document) = match pending_kept {
        Some(kept) => (
            kept["fields"]["host"].clone(),
            json!({ "reused": pending }),
            kept,
        ),
        None => {
            let placed = super::super::fleet::place_on(
                name,
                super::super::DEFAULT_ENGINE,
                host,
                port,
                Some(major.as_str()),
            )
            .await;
            let written = document(&owner, &item).await?;
            let fleet_written =
                provider(&item, &written["fields"])? == Some(DatabaseProvider::Fleet);
            match placed {
                // `place` stores the item before it installs the unit, so a
                // placement that failed after the store left <item> on a
                // server that may not run: the source's coordinates go back
                // and the fleet ones are kept for a second run.
                Err(error) => {
                    if fleet_written {
                        keep_copy(&owner, &pending, &item, &written).await?;
                        store(&owner, &item, &source_document).await?;
                        return Err(error.also(format!(
                            "{item} holds the source's coordinates again and the fleet ones are {pending}"
                        )));
                    }
                    return Err(error);
                }
                Ok((placed_on, placement)) => {
                    if !fleet_written {
                        return Err(CmdError::click(format!(
                            "{name} was placed on {placed_on} ({placement}) but {item} still holds the source's coordinates: \
                             a fleet server from an earlier placement stands there with no item holding its password"
                        ))
                        .stating(FailureCode::Config));
                    }
                    (json!(placed_on), placement, written)
                }
            }
        }
    };
    let target = Connection::from_item(&item, &target_document["fields"])?;
    let restored = match restore::restore(&backup, &target) {
        Ok(report) if restore::complete(&report) => report,
        outcome => {
            keep_copy(&owner, &pending, &item, &target_document).await?;
            store(&owner, &item, &source_document).await?;
            let cause = match outcome {
                Ok(report) => format!("tables differ after the restore: {}", report["mismatches"]),
                Err(error) => error.to_string(),
            };
            return Err(CmdError::click(format!(
                "{name} was not moved: {cause}. {item} holds the source's coordinates again; the fleet server on \
                 {placed_on} keeps the partial copy and its coordinates are {pending}, which a second run restores into; \
                 the backup is {}",
                backup.directory.display()
            ))
            .stating(FailureCode::InfraDown));
        }
    };
    store(&owner, &item, &target_document).await?;
    let pending_status = if from_pending {
        owner.delete(&pending).await?
    } else {
        "never written"
    };
    let report = json!({
        "migrated": name,
        "from": backup.manifest["source"],
        "from_provider": fields.get("provider"),
        "project_ref": fields.get("project_ref"),
        "to": target.address(),
        "host": placed_on,
        "postgres_version": major,
        "placement": placement,
        "backup": backup.directory.display().to_string(),
        "backup_taken_at": backup.manifest["created_at"],
        "restore": restored,
        "item": item,
        "previous_item": previous,
        "pending_item": pending_status,
        "consumers": "unchanged: each reads the fleet coordinates from the item on its next connection; \
                      the source's API keys and project URL stay only on the previous item",
        "rest_api": format!(
            "a consumer that called the provider's REST API keeps calling it until \
             `stado database rest place {name} --port <port> --key-valid-days <days>` serves it from the fleet"
        ),
    });
    if json_output {
        println!("{}", serde_json::to_string_pretty(&report)?);
        return Ok(());
    }
    crate::cli::print_answer(&report, false)
}

/// A kept backup is restored only when the source no longer answers: while it
/// answers, a new backup (`migrate` without `--from-backup`) holds every write
/// the kept one lacks, and restoring the kept one would drop them. The source
/// is asked for its server version through the item's own session; an item
/// that names no session the source could be reached on counts as silent.
fn source_silent(name: &str, item: &str, fields: &Value, directory: &std::path::Path) -> Result<(), CmdError> {
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
    let answered = super::tools::server_major(&connection, ca_file.as_deref());
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
