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
mod fleet;
mod reads;
mod supabase;
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
        DatabaseCommands::Create {
            name,
            consumers,
            engine,
            provider,
            host,
            port,
            anchor,
            accept_monthly_usd,
            json,
        } => match provider.as_str() {
            "fleet" => {
                if anchor.is_some() || accept_monthly_usd.is_some() {
                    return Err(CmdError::usage(
                        "--anchor and --accept-monthly-usd price a supabase project; a fleet database has no vendor bill",
                    ));
                }
                fleet::create(&name, &engine, host.as_deref(), port, &consumers, json).await
            }
            "supabase" => {
                if engine != "postgres" {
                    return Err(CmdError::usage(format!(
                        "supabase runs postgres only; --engine {engine} is created with --provider fleet"
                    )));
                }
                if host.is_some() || port.is_some() {
                    return Err(CmdError::usage(
                        "--host and --port place a fleet database; supabase chooses its own",
                    ));
                }
                let anchor = anchor.as_deref().unwrap_or(supabase::DEFAULT_ANCHOR);
                supabase::create::create(&name, anchor, &consumers, accept_monthly_usd, json).await
            }
            other => Err(CmdError::usage(format!(
                "--provider must be fleet or supabase, got {other:?}"
            ))),
        },
        DatabaseCommands::Place {
            name,
            engine,
            port,
            json,
        } => fleet::place(&name, &engine, port, json).await,
        DatabaseCommands::Adopt {
            name,
            project_ref,
            password_file,
            check,
            json,
        } => {
            supabase::adopt::adopt(
                name.as_deref(),
                project_ref.as_deref(),
                password_file.as_deref(),
                check,
                json,
            )
            .await
        }
        DatabaseCommands::Push {
            host,
            service,
            check,
            json,
        } => writes::push(&host, &service, check, json).await,
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
