//! `stado placement profile`: list, show, declare and remove the placement
//! profiles the registry holds, the declarations `placement standby` adds a
//! host to and `placement move` moves.

mod document;

use std::io::Read;
use std::path::Path;

use clap::Subcommand;
use serde_json::{json, Value};

use crate::cli::{registry, CmdError};

#[derive(Subcommand)]
pub enum ProfileCommands {
    /// Every placement profile: its services and the hosts it may run on.
    List {
        #[arg(long)]
        json: bool,
    },
    /// One placement profile exactly as the registry holds it.
    Show { name: String },
    /// Add a placement profile, or replace the one of the same name, from a
    /// JSON file (`-` reads standard input), and link each of its services
    /// to it in the service directory, in one registry commit. A service
    /// another profile holds, a service the directory does not declare, a
    /// service an existing profile of that name would lose, and a profile a
    /// placement move holds are refused before anything is written.
    Declare {
        /// The profile JSON: name, services, start_order, stop_order, state,
        /// hosts (units and probes per host), and optionally routing and
        /// allow_unhealthy_source.
        #[arg(long)]
        file: String,
        #[arg(long)]
        json: bool,
    },
    /// Remove a placement profile; each of its services becomes a fixed route
    /// naming the unit the profile declares on its active host.
    Remove {
        name: String,
        #[arg(long)]
        json: bool,
    },
}

fn read_profile(file: &str) -> Result<Value, CmdError> {
    let text = if file == "-" {
        let mut text = String::new();
        std::io::stdin()
            .read_to_string(&mut text)
            .map_err(|error| {
                CmdError::click(format!("standard input could not be read: {error}"))
            })?;
        text
    } else {
        std::fs::read_to_string(Path::new(file)).map_err(|error| {
            CmdError::missing(format!(
                "the profile file {file} could not be read: {error}"
            ))
        })?
    };
    serde_json::from_str(&text).map_err(|error| {
        CmdError::declaration(format!("the profile in {file} is not JSON: {error}"))
    })
}

fn declared_profiles(document: &Value) -> Result<Vec<Value>, CmdError> {
    match document.get("placement_profiles") {
        None => Ok(Vec::new()),
        Some(value) => value
            .as_array()
            .cloned()
            .ok_or_else(|| CmdError::declaration("registry.placement_profiles is not an array")),
    }
}

fn summary(profile: &Value) -> Result<Value, CmdError> {
    let name = profile
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| CmdError::declaration("a registry placement profile carries no name"))?;
    let hosts = profile
        .get("hosts")
        .and_then(Value::as_object)
        .ok_or_else(|| {
            CmdError::declaration(format!("placement profile {name} hosts is not an object"))
        })?;
    let hosts: Vec<&String> = hosts.keys().collect();
    Ok(json!({ "name": name, "services": profile.get("services"), "hosts": hosts }))
}

async fn list(json_output: bool) -> Result<(), CmdError> {
    let document = registry::fetch_document().await?;
    let rows = declared_profiles(&document)?
        .iter()
        .map(summary)
        .collect::<Result<Vec<Value>, CmdError>>()?;
    if json_output {
        println!("{}", serde_json::to_string_pretty(&rows)?);
        return Ok(());
    }
    if rows.is_empty() {
        println!("the registry declares no placement profile");
    }
    for row in &rows {
        println!("{}", serde_json::to_string(row)?);
    }
    Ok(())
}

async fn show(name: &str) -> Result<(), CmdError> {
    let document = registry::fetch_document().await?;
    let profile = declared_profiles(&document)?
        .into_iter()
        .find(|profile| profile.get("name").and_then(Value::as_str) == Some(name))
        .ok_or_else(|| {
            CmdError::missing(format!(
                "the registry declares no placement profile named {name}"
            ))
        })?;
    println!("{}", serde_json::to_string_pretty(&profile)?);
    Ok(())
}

fn receipt(action: &str, name: &str, generation: &str, json_output: bool) -> Result<(), CmdError> {
    if json_output {
        let answer = json!({ "profile": name, "state": action, "registry_generation": generation });
        println!("{}", serde_json::to_string_pretty(&answer)?);
    } else {
        println!("{action} placement profile {name} (registry generation {generation})");
    }
    Ok(())
}

pub async fn dispatch(command: ProfileCommands) -> Result<(), CmdError> {
    match command {
        ProfileCommands::List { json } => list(json).await,
        ProfileCommands::Show { name } => show(&name).await,
        ProfileCommands::Declare { file, json } => {
            let candidate = read_profile(&file)?;
            let name = document::parse(&candidate)?.name;
            let generation =
                registry::commit_document(|current| document::declare(current, &candidate)).await?;
            receipt("declared", &name, &generation, json)
        }
        ProfileCommands::Remove { name, json } => {
            let generation =
                registry::commit_document(|current| document::remove(current, &name)).await?;
            receipt("removed", &name, &generation, json)
        }
    }
}
