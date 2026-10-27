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

use super::resolver::read_local_document;
use super::CmdError;

mod client;
mod commands;
mod dump;
mod external;
mod fleet;
mod reads;
mod rest;
mod supabase;
mod verbs;
mod writes;

pub(crate) use self::commands::DatabaseCommands;
pub(crate) use self::dump::files::BackupsArgs;
pub(crate) use self::dump::migrate::MigrateArgs;
pub(crate) use self::dump::BackupArgs;

use self::reads::{list, resolve};
use self::verbs::{change_consumers, declare, remove};

/// Who runs a database: what `--provider` selects, and what every credential
/// item records in its `provider` field.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize, clap::ValueEnum,
)]
#[serde(rename_all = "lowercase")]
#[value(rename_all = "lowercase")]
pub(crate) enum DatabaseProvider {
    /// Stado, on a fleet host.
    Fleet,
    /// A hosted Supabase project.
    Supabase,
    /// An existing server of any engine Stado does not run.
    External,
}

/// The engine a fleet or Supabase database runs when `--engine` names none,
/// as the database page's create table declares it: the one engine Supabase
/// runs and the fleet's default.
const DEFAULT_ENGINE: &str = "postgres";

impl std::fmt::Display for DatabaseProvider {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        crate::primitives::word::write(self, formatter)
    }
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
        DatabaseCommands::Destroy { name, host, delete_project, json } => {
            fleet::destroy(&name, host.as_deref(), delete_project, json).await
        }
        DatabaseCommands::Create {
            name,
            consumers,
            engine,
            provider,
            ca_certificate,
            host,
            port,
            anchor,
            accept_monthly_usd,
            json,
        } => match provider {
            _ if provider != DatabaseProvider::External && ca_certificate.is_some() => {
                Err(CmdError::usage(
                    "--ca-certificate names an external server's authority; fleet and supabase databases carry their own",
                ))
            }
            DatabaseProvider::External => {
                if host.is_some() || port.is_some() || anchor.is_some() || accept_monthly_usd.is_some() {
                    return Err(CmdError::usage(
                        "--provider external takes only --ca-certificate and the connection URL on standard input; the server is already placed",
                    ));
                }
                let Some(ca_certificate) = ca_certificate else {
                    return Err(CmdError::usage(
                        "--provider external needs --ca-certificate: consumers verify the server against it",
                    ));
                };
                external::create(&name, engine.as_deref(), &ca_certificate, &consumers, json).await
            }
            DatabaseProvider::Fleet => {
                if anchor.is_some() || accept_monthly_usd.is_some() {
                    return Err(CmdError::usage(
                        "--anchor and --accept-monthly-usd price a supabase project; a fleet database has no vendor bill",
                    ));
                }
                let engine = match engine.as_deref() {
                    Some(engine) => engine,
                    None => DEFAULT_ENGINE,
                };
                fleet::create(&name, engine, host.as_deref(), port, &consumers, json).await
            }
            DatabaseProvider::Supabase => {
                if let Some(engine) = engine.as_deref().filter(|engine| *engine != DEFAULT_ENGINE) {
                    return Err(CmdError::usage(format!(
                        "supabase runs postgres only; --engine {engine} is created with --provider fleet or external"
                    )));
                }
                if host.is_some() || port.is_some() {
                    return Err(CmdError::usage(
                        "--host and --port place a fleet database; supabase chooses its own",
                    ));
                }
                supabase::create::create(
                    &name,
                    anchor.as_deref(),
                    &consumers,
                    accept_monthly_usd,
                    json,
                )
                .await
            }
        },
        DatabaseCommands::Place {
            name,
            engine,
            port,
            version,
            json,
        } => fleet::place(&name, &engine, port, version.as_deref(), json).await,
        DatabaseCommands::Backup(args) => dump::backup(args.name.as_deref(), args.keep, args.json).await,
        DatabaseCommands::Backups(args) => dump::files::list(args.name.as_deref(), args.json),
        DatabaseCommands::Migrate(args) => dump::migrate::run(args).await,
        DatabaseCommands::Rest { command } => match command {
            rest::RestCommands::Place(args) => rest::place(args).await,
            rest::RestCommands::Retire(args) => rest::retire::retire(args).await,
        },
        DatabaseCommands::Cost { provider, json } => match provider {
            DatabaseProvider::Supabase => supabase::cost::cost(json).await,
            DatabaseProvider::Fleet => Err(CmdError::usage(
                "--provider fleet has no bill to read: a fleet database runs on a fleet host with no vendor",
            )),
            DatabaseProvider::External => Err(CmdError::usage(
                "--provider external has no bill Stado can read: the server is one Stado does not run",
            )),
        },
        DatabaseCommands::Adopt {
            name,
            project_ref,
            password_file,
            rotate_password,
            check,
            json,
        } => {
            let password = match (password_file, rotate_password) {
                (_, true) => supabase::adopt::Password::Rotate,
                (Some(path), false) => supabase::adopt::Password::File(path),
                (None, false) => supabase::adopt::Password::Kept,
            };
            supabase::adopt::adopt(name.as_deref(), project_ref.as_deref(), password, check, json)
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
        DatabaseCommands::Client { name, json } => client::client(&name, json).await,
    }
}

/// The declared databases. A `database_api` section (or
/// `WC_DATABASE_API_DATABASES`) that does not parse is the operator's
/// configuration naming something invalid, so it is stated as a configuration
/// failure, never left to read as an unattributed failure of Stado.
pub(in crate::cli) fn declared_databases(
) -> Result<&'static std::collections::BTreeMap<String, crate::config::DatabaseApiDatabase>, CmdError>
{
    crate::config::database_api_databases().map_err(|problems| {
        CmdError::click(problems.join("; "))
            .stating(crate::primitives::failure::FailureCode::Config)
    })
}

async fn registry_document() -> Result<Value, CmdError> {
    let store = RegistryStore::open().await?;
    let (bootstrap, _) = read_local_document(&store).await?;
    crate::targets::validate_registry(&bootstrap).map_err(CmdError::from)?;
    if bootstrap.get("service_directory").is_none() {
        return Ok(bootstrap);
    }
    let target = super::resolver::current_target(&bootstrap).map_err(CmdError::declaration)?;
    super::resolver::directory_document(Arc::new(store), &bootstrap, &target).await
}

fn directory_routes(document: &Value) -> Result<&serde_json::Map<String, Value>, CmdError> {
    let routes = document
        .get("service_directory")
        .and_then(|directory| directory.get("services"))
        .and_then(Value::as_object)
        .ok_or_else(|| {
            CmdError::click("registry carries no service_directory")
                .stating(crate::primitives::failure::FailureCode::Config)
        })?;
    Ok(routes)
}
