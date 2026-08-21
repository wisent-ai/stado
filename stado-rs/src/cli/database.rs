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
    /// Returns the placement endpoint and the Skarbiec item to acquire the
    /// credential from. The consumer must be declared on the database; the
    /// credential value is never printed.
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
}

pub(crate) async fn dispatch(command: DatabaseCommands) -> Result<(), CmdError> {
    match command {
        DatabaseCommands::List { json } => list(json).await,
        DatabaseCommands::Resolve {
            name,
            consumer,
            json,
        } => resolve(&name, &consumer, json).await,
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

    let document = registry_document().await?;
    let resolved =
        service_resolution::resolve(&document, name, consumer).map_err(CmdError::click)?;

    let report = json!({
        "database": format!("stado://database/{}", resolved.name),
        "engine": database.engine(),
        "scopes": database.scopes(),
        "credential_item": database.item(),
        "generation": resolved.generation,
        "active_host": resolved.active_host,
        "endpoint": resolved.endpoint.url,
        "capabilities": resolved.capabilities,
    });
    if json_output {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!(
            "{} engine={} endpoint={} item={} scopes={}",
            report["database"].as_str().unwrap_or_default(),
            database.engine(),
            resolved.endpoint.url,
            database.item(),
            database.scopes().join(","),
        );
    }
    Ok(())
}
