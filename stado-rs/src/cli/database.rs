//! `stado database` — the fleet's database plane.
//!
//! The object plane answers "where are the bytes"; this plane answers "where
//! is the database". A database is declared once in `database_api.databases`
//! (engine, scopes, the Skarbiec item holding its credential, the consumers
//! allowed to resolve it) and placed like any other service through the
//! service directory. Resolution hands out the endpoint and the credential
//! coordinate; the secret itself never passes through this surface.

use clap::Subcommand;
use serde_json::{json, Value};
use std::sync::Arc;

use crate::service_resolution;
use crate::targets::RegistryStore;

use super::resolver::read_local_snapshot;
use super::CmdError;

#[derive(Debug, Subcommand)]
pub(crate) enum DatabaseCommands {
    /// List declared databases and whether each is placed.
    List {
        /// Emit machine-readable output.
        #[arg(long)]
        json: bool,
    },
    /// Resolve one database for an authorized consumer.
    ///
    /// Returns the placement endpoint when the service directory places the
    /// database, and always the Skarbiec item to acquire the credential
    /// from. The consumer must be declared on the database; the credential
    /// value is never printed.
    Resolve {
        /// Logical database name from database_api.databases.
        name: String,
        /// Stable workload identity requesting access.
        #[arg(long)]
        consumer: String,
        /// Emit machine-readable output.
        #[arg(long)]
        json: bool,
    },
    /// Declare a database in the Stado configuration.
    ///
    /// Writes `database_api.databases.<name>` through the same validated,
    /// atomic write every other configuration change uses. The credential
    /// item `<name>-database` is implied; provision its fields with
    /// `stado secrets put <name>-database`.
    Declare {
        /// Logical database name (lowercase letters, digits, dashes).
        name: String,
        /// Database engine.
        #[arg(long)]
        engine: String,
        /// Access scopes to grant the declaration (read, write).
        #[arg(long = "scope", value_delimiter = ',')]
        scopes: Vec<String>,
        /// Consumer allowed to resolve this database.
        #[arg(long = "consumer", value_delimiter = ',', required = true)]
        consumers: Vec<String>,
        /// Emit machine-readable output.
        #[arg(long)]
        json: bool,
    },
    /// Remove a database declaration from the Stado configuration.
    Remove {
        name: String,
        /// Emit machine-readable output.
        #[arg(long)]
        json: bool,
    },
    /// Grant one or more consumers access to a declared database.
    Grant {
        name: String,
        #[arg(long = "consumer", value_delimiter = ',', required = true)]
        consumers: Vec<String>,
        /// Emit machine-readable output.
        #[arg(long)]
        json: bool,
    },
    /// Revoke one or more consumers' access to a declared database.
    Revoke {
        name: String,
        #[arg(long = "consumer", value_delimiter = ',', required = true)]
        consumers: Vec<String>,
        /// Emit machine-readable output.
        #[arg(long)]
        json: bool,
    },
}

pub(crate) async fn dispatch(command: DatabaseCommands) -> Result<(), CmdError> {
    match command {
        DatabaseCommands::List { json } => list(json).await,
        DatabaseCommands::Resolve {
            name,
            consumer,
            json,
        } => resolve(&name, &consumer, json).await,
        DatabaseCommands::Declare {
            name,
            engine,
            scopes,
            consumers,
            json,
        } => declare(&name, &engine, &scopes, &consumers, json),
        DatabaseCommands::Remove { name, json } => remove(&name, json),
        DatabaseCommands::Grant {
            name,
            consumers,
            json,
        } => change_consumers(&name, &consumers, true, json),
        DatabaseCommands::Revoke {
            name,
            consumers,
            json,
        } => change_consumers(&name, &consumers, false, json),
    }
}

async fn registry_document() -> Result<Value, CmdError> {
    let store = Arc::new(RegistryStore::open().await?);
    let (bootstrap, _, _) = read_local_snapshot(&store).await.map_err(CmdError::click)?;
    let target = super::resolver::current_target(&bootstrap).map_err(CmdError::click)?;
    let source = super::resolver::snapshot_source(store, &bootstrap, &target).map_err(CmdError::click)?;
    let (document, _, _) = source
        .fetch(crate::monitor::host_silence::READER_CLI)
        .await
        .map_err(CmdError::click)?;
    Ok(document)
}

fn directory_routes(document: &Value) -> Result<&serde_json::Map<String, Value>, CmdError> {
    let routes = document
        .get("service_directory")
        .and_then(|directory| directory.get("services"))
        .and_then(Value::as_object)
        .ok_or_else(|| CmdError::click("registry carries no service_directory"))?;
    Ok(routes)
}
async fn list(json_output: bool) -> Result<(), CmdError> {
    let databases = crate::config::database_api_databases()
        .map_err(|problems| CmdError::click(problems.join("; ")))?;
    let document = registry_document().await?;
    let routes = directory_routes(&document)?;

    let mut rows = Vec::new();
    for (name, database) in databases {
        let route = routes.get(name);
        rows.push(json!({
            "database": name,
            "engine": database.engine(),
            "item": database.item(),
            "scopes": database.scopes(),
            "consumers": database.consumers(),
            "placed": route.is_some(),
            "active_host": route.and_then(|route| route.get("active_host")).cloned(),
        }));
    }

    if json_output {
        println!("{}", serde_json::to_string_pretty(&rows)?);
    } else {
        for row in &rows {
            let scopes = row["scopes"].as_array().map(|values| {
                values.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(",")
            }).unwrap_or_default();
            let consumers = row["consumers"].as_array().map(|values| {
                values.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(",")
            }).unwrap_or_default();
            println!(
                "{} engine={} item={} scopes=[{scopes}] consumers=[{consumers}] placed={}",
                row["database"].as_str().unwrap_or_default(),
                row["engine"].as_str().unwrap_or_default(),
                row["item"].as_str().unwrap_or_default(),
                row["placed"].as_bool().map(|placed| placed.to_string()).unwrap_or_default(),
            );
        }
    }
    Ok(())
}

async fn resolve(name: &str, consumer: &str, json_output: bool) -> Result<(), CmdError> {
    let databases = crate::config::database_api_databases()
        .map_err(|problems| CmdError::click(problems.join("; ")))?;
    let database = databases.get(name).ok_or_else(|| {
        CmdError::usage(format!(
            "unknown database {name:?}; declared: {}",
            databases.keys().cloned().collect::<Vec<_>>().join(", ")
        ))
    })?;
    if !database.allows_consumer(consumer) {
        return Err(CmdError::usage(format!(
            "consumer {consumer:?} is not authorized for database {name:?}"
        )));
    }

    // Placement is optional. A declared database whose service-directory
    // route does not exist yet still resolves: the consumer learns the
    // credential coordinate and takes the endpoint from the credential
    // itself. Requiring a route here would make every fresh declaration
    // unresolvable until someone edited the canonical registry by hand,
    // which is exactly the one-off this plane exists to replace.
    let document = registry_document().await?;
    let placed = document
        .get("service_directory")
        .and_then(|directory| directory.get("services"))
        .and_then(|services| services.get(name))
        .is_some();

    let mut report = json!({
        "database": format!("stado://database/{name}"),
        "engine": database.engine(),
        "scopes": database.scopes(),
        "credential_item": database.item(),
        "placed": placed,
    });
    if placed {
        let resolved =
            service_resolution::resolve(&document, name, consumer).map_err(CmdError::click)?;
        report["generation"] = json!(resolved.generation);
        report["active_host"] = json!(resolved.active_host);
        report["endpoint"] = json!(resolved.endpoint.url);
        report["capabilities"] = json!(resolved.capabilities);
    }
    if json_output {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!(
            "{} engine={} item={} scopes={} placed={}",
            report["database"].as_str().unwrap_or_default(),
            database.engine(),
            database.item(),
            database.scopes().join(","),
            placed,
        );
    }
    Ok(())
}

/// Load the config file, apply one mutation to `database_api.databases`,
/// refuse anything the plane's own parser rejects, and write atomically.
fn mutate_databases<F>(mutation: F) -> Result<Value, CmdError>
where
    F: FnOnce(&mut serde_json::Map<String, Value>) -> Result<(), String>,
{
    let path = crate::config_file::config_path()
        .map_err(|error| CmdError::click(error.to_string()))?
        .ok_or_else(|| CmdError::click("no config file exists; run: stado config init"))?;
    let original = std::fs::read_to_string(&path)?;
    let mut document: Value =
        serde_json::from_str(&original).map_err(|error| CmdError::click(error.to_string()))?;
    if !document.is_object() {
        return Err(CmdError::click("config file must contain a JSON object"));
    }

    let entry = document
        .as_object_mut()
        .expect("checked above")
        .entry("database_api".to_string())
        .or_insert_with(|| json!({}));
    if !entry.is_object() {
        return Err(CmdError::click("database_api must be an object"));
    }
    let databases = entry
        .as_object_mut()
        .expect("checked above")
        .entry("databases".to_string())
        .or_insert_with(|| json!({}));
    let map = databases
        .as_object_mut()
        .ok_or_else(|| CmdError::click("database_api.databases must be an object"))?;
    mutation(map)?;
    // The parser refuses an empty map, so a removal that empties the plane
    // collapses the section instead of leaving a configuration nothing can
    // validate.
    if map.is_empty() {
        document
            .as_object_mut()
            .expect("checked above")
            .remove("database_api");
    }

    // The plane's parser is the authority on shape; run it before the whole
    // document's validation so the refusal names the database, not an
    // unrelated section the generic validator happened to reach first. A
    // document that no longer carries the section at all has nothing for
    // this plane to reject.
    if let Some(databases) = document
        .get("database_api")
        .and_then(|section| section.get("databases"))
    {
        if let Err(problems) = crate::config::parse_database_api_databases(Some(databases)) {
            return Err(CmdError::click(format!(
                "rejected, config unchanged: {}",
                problems.join("; ")
            )));
        }
    }

    let problems = crate::config_file::validate(&document);
    if !problems.is_empty() {
        return Err(CmdError::click(format!(
            "rejected, config unchanged: {}",
            problems.join("; ")
        )));
    }

    let body = format!("{}\n", serde_json::to_string_pretty(&document)?);
    let temporary = std::path::PathBuf::from(format!("{}.database-setting", path.display()));
    std::fs::write(&temporary, body)?;
    if let Ok(metadata) = std::fs::metadata(&path) {
        std::fs::set_permissions(&temporary, metadata.permissions())?;
    }
    std::fs::rename(&temporary, &path)?;
    Ok(document)
}

fn canonical_name(name: &str) -> bool {
    !name.is_empty()
        && name.trim() == name
        && name
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

fn declare(
    name: &str,
    engine: &str,
    scopes: &[String],
    consumers: &[String],
    json_output: bool,
) -> Result<(), CmdError> {
    if !canonical_name(name) {
        return Err(CmdError::usage(
            "NAME must be lowercase letters, digits and dashes",
        ));
    }
    if !crate::config::DATABASE_API_ENGINES.contains(&engine) {
        return Err(CmdError::usage(format!(
            "engine must be one of {:?}",
            crate::config::DATABASE_API_ENGINES
        )));
    }
    let mut clean_scopes: Vec<String> = scopes.to_vec();
    if clean_scopes.is_empty() {
        clean_scopes.push("read".to_string());
    }
    for scope in &clean_scopes {
        if !crate::config::DATABASE_API_SCOPES.contains(&scope.as_str()) {
            return Err(CmdError::usage(format!(
                "scope {scope:?} is not one of {:?}",
                crate::config::DATABASE_API_SCOPES
            )));
        }
    }
    let mut seen = std::collections::BTreeSet::new();
    let mut clean_consumers = Vec::new();
    for consumer in consumers {
        let consumer = consumer.trim();
        if !canonical_name(consumer) {
            return Err(CmdError::usage(format!(
                "consumer {consumer:?} must be lowercase letters, digits and dashes"
            )));
        }
        if seen.insert(consumer.to_string()) {
            clean_consumers.push(consumer.to_string());
        }
    }

    let declaration = json!({
        "engine": engine,
        "scopes": clean_scopes,
        "consumers": clean_consumers,
    });
    mutate_databases(|map| {
        map.insert(name.to_string(), declaration.clone());
        Ok(())
    })?;
    report_mutation(
        json_output,
        json!({
            "declared": name,
            "engine": engine,
            "scopes": clean_scopes,
            "consumers": clean_consumers,
            "item": format!("{name}-database"),
        }),
    )
}

fn remove(name: &str, json_output: bool) -> Result<(), CmdError> {
    mutate_databases(|map| {
        if map.remove(name).is_none() {
            return Err(format!("database {name:?} is not declared"));
        }
        Ok(())
    })?;
    report_mutation(json_output, json!({"removed": name}))
}

fn change_consumers(
    name: &str,
    consumers: &[String],
    grant: bool,
    json_output: bool,
) -> Result<(), CmdError> {
    for consumer in consumers {
        let consumer = consumer.trim();
        if !canonical_name(consumer) {
            return Err(CmdError::usage(format!(
                "consumer {consumer:?} must be lowercase letters, digits and dashes"
            )));
        }
    }
    let mut changed = Vec::new();
    mutate_databases(|map| {
        let declaration = map
            .get_mut(name)
            .ok_or_else(|| format!("database {name:?} is not declared"))?;
        let list = declaration
            .as_object_mut()
            .ok_or_else(|| format!("database {name:?} is malformed"))?
            .entry("consumers")
            .or_insert_with(|| json!([]));
        let list = list
            .as_array_mut()
            .ok_or_else(|| format!("database {name:?}.consumers must be an array"))?;
        for consumer in consumers {
            let consumer = consumer.trim().to_string();
            let entry = Value::String(consumer.clone());
            if grant {
                if !list.contains(&entry) {
                    list.push(entry);
                    changed.push(consumer);
                }
            } else {
                if list.len() == 1 && list[0] == entry {
                    return Err(format!(
                        "cannot revoke the last consumer of {name:?}; remove the declaration instead"
                    ));
                }
                if let Some(position) = list.iter().position(|existing| existing == &entry) {
                    list.remove(position);
                    changed.push(consumer);
                }
            }
        }
        Ok(())
    })?;
    report_mutation(
        json_output,
        json!({
            "database": name,
            "granted": if grant { changed.clone() } else { Vec::<String>::new() },
            "revoked": if grant { Vec::<String>::new() } else { changed },
        }),
    )
}

fn report_mutation(json_output: bool, report: Value) -> Result<(), CmdError> {
    let _ = json_output;
    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
}
