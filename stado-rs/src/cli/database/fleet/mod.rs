//! `stado database create --provider fleet` and `stado database place`: a
//! database Stado runs itself.
//!
//! No vendor, no bill and no question for the operator. The database lives on
//! one fleet host -- the vault owner unless `--host` names another -- and
//! `create` runs `place` there through the host channel. `place` initialises
//! the engine's data under `~/.stado/databases/<name>/` and writes the
//! credential item `<name>-database` into the owner vault; for postgres it
//! also installs the managed unit `<name>-database` that serves it.
//!
//! SQLite has no listener: its item names the host and the file, and only a
//! consumer on that host can open it. Postgres is served over TLS; see
//! [`postgres`].

use serde_json::{json, Value};
use std::path::{Path, PathBuf};

use crate::cli::CmdError;

use super::supabase::owner_vault;

mod destroy;
mod postgres;

pub(super) use destroy::destroy;

pub(super) async fn create(
    name: &str,
    engine: &str,
    host: Option<&str>,
    port: Option<u16>,
    consumers: &[String],
    json_output: bool,
) -> Result<(), CmdError> {
    checked(name, engine)?;
    let (owner, here) = crate::cli::release_catalog::fleet_hosts().await?;
    let host = host.map(str::to_string).unwrap_or(owner);
    let placed = if host == here {
        placement(name, engine, port, &here).await?
    } else {
        // NAME and ENGINE are checked tokens without spaces, so the command
        // line splits back into exactly the words it was written from.
        let mut line = format!("database place {name} --engine {engine} --json");
        if let Some(port) = port {
            line.push_str(&format!(" --port {port}"));
        }
        let arguments: Vec<&str> = line.split_whitespace().collect();
        let output = crate::cli::host::remote_stado_output(&host, &arguments)
            .await
            .map_err(|error| {
                CmdError::click(format!("{name} was not placed on {host}: {error}"))
            })?;
        last_json(&output).ok_or_else(|| {
            CmdError::click(format!(
                "{name}: stado database place on {host} printed no placement report: {}",
                output.trim()
            ))
        })?
    };
    let declared = super::verbs::declaration(
        name,
        engine,
        &["read".to_string(), "write".to_string()],
        consumers,
    )?;
    let outcome = json!({
        "created": name,
        "provider": "fleet",
        "engine": engine,
        "host": host,
        "placement": placed,
        "declaration": declared,
    });
    if json_output {
        println!("{}", serde_json::to_string_pretty(&outcome)?);
    } else {
        println!(
            "database {name}: {engine} on {host}, item {name}-database; {}",
            if placed["reused"] == true {
                "already placed there, reused"
            } else {
                "placed"
            }
        );
    }
    Ok(())
}

pub(super) async fn place(
    name: &str,
    engine: &str,
    port: Option<u16>,
    json_output: bool,
) -> Result<(), CmdError> {
    checked(name, engine)?;
    let here = crate::cli::release_catalog::this_host().await?;
    let report = placement(name, engine, port, &here).await?;
    // `--json` stays one line: `create` reads the last line of a remote
    // placement's output (`last_json`), after whatever the unit install printed.
    if json_output {
        println!("{}", serde_json::to_string(&report)?);
        return Ok(());
    }
    crate::cli::print_answer(&report, false)
}

fn checked(name: &str, engine: &str) -> Result<(), CmdError> {
    if !super::writes::canonical_name(name) {
        return Err(CmdError::usage(
            "NAME must be lowercase letters, digits and dashes",
        ));
    }
    use crate::config::DatabaseEngine;
    match DatabaseEngine::parse(engine) {
        Some(DatabaseEngine::Postgres | DatabaseEngine::Sqlite) => {}
        _ if DatabaseEngine::is_engine_name(engine) => {
            return Err(CmdError::usage(format!(
                "a fleet database runs postgres or sqlite; an existing {engine} server is brought in with --provider external"
            )))
        }
        _ => {
            return Err(CmdError::usage(format!(
                "engine {engine:?} is not an engine name: the lowercase scheme of its connection URL, such as postgres, mysql or mongodb"
            )))
        }
    }
    Ok(())
}

/// The last line of `output` that is a JSON object: the report `place`
/// prints after whatever the unit install printed before it.
fn last_json(output: &str) -> Option<Value> {
    output
        .lines()
        .rev()
        .find_map(|line| serde_json::from_str::<Value>(line.trim()).ok())
        .filter(Value::is_object)
}

/// Place `name` on this host, which the registry calls `here`.
async fn placement(
    name: &str,
    engine: &str,
    port: Option<u16>,
    here: &str,
) -> Result<Value, CmdError> {
    let home = std::env::var_os("HOME")
        .filter(|home| !home.is_empty())
        .map(PathBuf::from)
        .ok_or_else(|| CmdError::click("HOME is not set; a fleet database lives under it"))?;
    let directory = home.join(".stado").join("databases").join(name);
    std::fs::create_dir_all(&directory)
        .map_err(|error| CmdError::click(format!("create {}: {error}", directory.display())))?;
    let owner = owner_vault::locate().await?;
    owner.ready()?;
    match engine {
        "sqlite" => place_sqlite(name, here, &directory, &owner).await,
        _ => postgres::place(name, here, &directory, port, &owner).await,
    }
}

async fn place_sqlite(
    name: &str,
    here: &str,
    directory: &Path,
    owner: &owner_vault::Owner,
) -> Result<Value, CmdError> {
    let file = directory.join(format!("{name}.sqlite3"));
    let reused = file.is_file();
    if !reused {
        // An empty file is a valid SQLite database; creating it here makes the
        // path the item names exist before any consumer opens it.
        std::fs::File::create(&file)
            .map_err(|error| CmdError::click(format!("create {}: {error}", file.display())))?;
    }
    let item = format!("{name}-database");
    let path = file.display().to_string();
    let fields = json!({ "engine": "sqlite", "provider": "fleet", "host": here, "path": path });
    let context = json!({ "engine": "sqlite", "provider": "fleet", "product": name });
    if let Err(error) = owner.store(&item, "bundle", &fields, &context).await {
        // A file this run created and no item names is a database nothing
        // declares and `destroy` cannot reach: take it back, and the
        // directory with it when nothing else is in there.
        if !reused {
            let _ = std::fs::remove_file(&file);
            let _ = std::fs::remove_dir(directory);
        }
        return Err(error);
    }
    Ok(json!({
        "reused": reused,
        "engine": "sqlite",
        "host": here,
        "path": path,
        "item": item,
        "item_vault": owner.name(),
    }))
}
