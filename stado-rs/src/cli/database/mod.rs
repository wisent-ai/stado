//! `stado database` — the fleet's database plane.
//!
//! The object plane answers "where are the bytes"; this plane answers "where
//! is the database". A database is declared once in `database_api.databases`
//! (engine, scopes, the Skarbiec item holding its credential, the consumers
//! allowed to resolve it) and placed like any other service through the
//! service directory. Resolution hands out the endpoint and the credential
//! coordinate; the secret itself never passes through this surface.

use serde_json::Value;
use std::sync::Arc;

use crate::targets::RegistryStore;

use super::resolver::read_local_snapshot;
use super::CmdError;

mod commands;
mod reads;
mod verbs;
mod writes;

pub(crate) use self::commands::DatabaseCommands;

use self::reads::{list, resolve};
use self::verbs::{change_consumers, declare, remove};

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
    let source = super::resolver::snapshot_source(Some(store), &bootstrap, &target)
        .map_err(CmdError::click)?;
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
