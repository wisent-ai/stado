//! `stado service directory` — the fleet's answer to "where is X, and who may
//! use it".
//!
//! The canonical registry grew a `service_directory` block that no source in
//! this tree modelled. It survived only because the registry write paths are
//! lossless: `push` uploads the operator's exact bytes and `push_document`
//! serializes a raw document. Nothing could read it, so every client that
//! needed a service address reconstructed one from a host name and a guess
//! about forwarded ports — which is wrong on every machine that is not the one
//! running the service.
//!
//! The block's shape, as the live document carries it:
//!
//! ```json
//! "service_directory": {
//!   "authority":  {"target": "...", "command": "..."},
//!   "generation": 1,
//!   "services": {
//!     "brama": {
//!       "placement_profile": "brama-skarbiec",
//!       "active_host": "charless-mac-mini",
//!       "endpoints": {"charless-mac-mini": {"url": "http://127.0.0.1:8080"},
//!                     "lukasz-macbook":    {"url": "http://127.0.0.1:8080"}},
//!       "consumers": {"operator": {"capabilities": ["model-routing"]}}
//!     }
//!   }
//! }
//! ```
//!
//! `endpoints` is keyed by the machine ASKING, not by the machine serving.
//! These services bind loopback on their own host, so "where is Brama" has a
//! different true answer per client and the directory states each one instead
//! of leaving every caller to derive it.
//!
//! Everything here reads and mutates the RAW document through
//! `registry::fetch_document` and `registry::push_document`. There is
//! deliberately no typed model of the block: a model is exactly what deletes
//! the keys it does not know, and this file exists because that already
//! happened to this document.

use clap::Subcommand;
use serde_json::{json, Map, Value};

use super::registry;
use crate::cli::CmdError;
use crate::targets;

const DIRECTORY_KEY: &str = "service_directory";

#[derive(Subcommand)]
pub enum DirectoryCommands {
    /// Print the whole service directory.
    Show {
        #[arg(long)]
        json: bool,
    },

    /// The address this machine should use for one service.
    ///
    /// Resolves against the asking target rather than the active host,
    /// because a loopback-bound service has a different address on every
    /// client. A target with no entry is reported as exactly that.
    Endpoint {
        /// Service name as the directory keys it, e.g. `brama`.
        name: String,
        /// Resolve as this target instead of this machine.
        #[arg(long)]
        target: Option<String>,
        #[arg(long)]
        json: bool,
    },

    /// Declare that a consumer may use a service.
    ConsumerAdd {
        /// Service name as the directory keys it.
        name: String,
        /// Consumer identity to declare.
        consumer: String,
        /// Capability to grant; repeat for several.
        #[arg(long = "capability")]
        capabilities: Vec<String>,
        #[arg(long)]
        json: bool,
    },

    /// Remove a consumer's declaration.
    ConsumerRm {
        /// Service name as the directory keys it.
        name: String,
        /// Consumer identity to remove.
        consumer: String,
        #[arg(long)]
        json: bool,
    },
}

fn click(message: impl std::fmt::Display) -> CmdError {
    CmdError::click(message.to_string())
}

/// The directory block, or a refusal naming what is absent. A missing block is
/// distinguished from an empty one: the first means nobody has ever declared
/// anything, the second that everything was withdrawn.
fn directory(document: &Value) -> Result<&Map<String, Value>, CmdError> {
    document
        .get(DIRECTORY_KEY)
        .and_then(Value::as_object)
        .ok_or_else(|| {
            click(format!(
                "the registry at {} carries no {DIRECTORY_KEY}",
                targets::registry_location()
            ))
        })
}

fn services(block: &Map<String, Value>) -> Result<&Map<String, Value>, CmdError> {
    block
        .get("services")
        .and_then(Value::as_object)
        .ok_or_else(|| click(format!("{DIRECTORY_KEY} carries no services map")))
}

fn service<'a>(block: &'a Map<String, Value>, name: &str) -> Result<&'a Value, CmdError> {
    let all = services(block)?;
    all.get(name).ok_or_else(|| {
        let known: Vec<&str> = all.keys().map(String::as_str).collect();
        click(format!(
            "no service {name:?} in {DIRECTORY_KEY}; it declares {}",
            known.join(", ")
        ))
    })
}

/// This machine's fleet name. The directory keys endpoints by target name, not
/// by hostname, so a hostname comparison would miss on every host whose fleet
/// name differs from its own idea of itself.
async fn this_target() -> Result<String, CmdError> {
    let hostname = crate::providers::vast::system_hostname();
    let registry = targets::fetch_registry_remote()
        .await
        .map_err(|exc| click(format!("cannot resolve this target: {exc}")))?;
    registry
        .lookup_self(&hostname)
        .map_err(|exc| click(exc.to_string()))?
        .map(|found| found.name.clone())
        .ok_or_else(|| {
            click(format!(
                "host {hostname} is not in {}",
                targets::registry_location()
            ))
        })
}

pub async fn dispatch(command: DirectoryCommands) -> Result<(), CmdError> {
    match command {
        DirectoryCommands::Show { json } => show(json).await,
        DirectoryCommands::Endpoint { name, target, json } => endpoint(&name, target, json).await,
        DirectoryCommands::ConsumerAdd {
            name,
            consumer,
            capabilities,
            json,
        } => consumer_add(&name, &consumer, capabilities, json).await,
        DirectoryCommands::ConsumerRm {
            name,
            consumer,
            json,
        } => consumer_rm(&name, &consumer, json).await,
    }
}

async fn show(as_json: bool) -> Result<(), CmdError> {
    let document = registry::fetch_document().await?;
    let block = directory(&document)?;
    if as_json {
        println!("{}", serde_json::to_string_pretty(block)?);
        return Ok(());
    }
    let all = services(block)?;
    for (name, entry) in all {
        let active = entry
            .get("active_host")
            .and_then(Value::as_str)
            .unwrap_or("unknown");
        println!("{name}  active_host={active}");
        if let Some(endpoints) = entry.get("endpoints").and_then(Value::as_object) {
            for (target, endpoint) in endpoints {
                let url = endpoint
                    .get("url")
                    .and_then(Value::as_str)
                    .unwrap_or("(no url)");
                println!("    from {target}: {url}");
            }
        }
        if let Some(consumers) = entry.get("consumers").and_then(Value::as_object) {
            let names: Vec<&str> = consumers.keys().map(String::as_str).collect();
            println!("    consumers: {}", names.join(", "));
        }
    }
    Ok(())
}

async fn endpoint(name: &str, target: Option<String>, as_json: bool) -> Result<(), CmdError> {
    let document = registry::fetch_document().await?;
    let block = directory(&document)?;
    let entry = service(block, name)?;
    let target = match target {
        Some(value) => value,
        None => this_target().await?,
    };
    let active = entry
        .get("active_host")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    let url = entry
        .get("endpoints")
        .and_then(Value::as_object)
        .and_then(|endpoints| endpoints.get(&target))
        .and_then(|endpoint| endpoint.get("url"))
        .and_then(Value::as_str);
    if as_json {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "service": name,
                "target": target,
                "active_host": active,
                "url": url,
            }))?
        );
        return Ok(());
    }
    match url {
        Some(url) => println!("{name} active on {active}, reached from {target} at {url}"),
        // Not a default and not an error: an undeclared endpoint means nobody
        // has said how this machine reaches the service, and inventing a
        // loopback address here is what sends a client to the wrong process.
        None => {
            println!("{name} active on {active}; {DIRECTORY_KEY} declares no endpoint for {target}")
        }
    }
    Ok(())
}

/// Mutate one service entry in place and write the whole document back.
///
/// The closure sees the service's own object, so nothing outside it can be
/// touched, and the write goes through `push_document`, which validates the
/// document and refuses one that would delete a top-level key.
async fn edit_service<F>(name: &str, edit: F) -> Result<String, CmdError>
where
    F: FnOnce(&mut Map<String, Value>) -> Result<(), CmdError>,
{
    let mut document = registry::fetch_document().await?;
    {
        let block = directory(&document)?;
        service(block, name)?;
    }
    let entry = document
        .get_mut(DIRECTORY_KEY)
        .and_then(Value::as_object_mut)
        .and_then(|block| block.get_mut("services"))
        .and_then(Value::as_object_mut)
        .and_then(|all| all.get_mut(name))
        .and_then(Value::as_object_mut)
        .ok_or_else(|| click(format!("service {name:?} is not an object")))?;
    edit(entry)?;
    registry::push_document(&document).await
}

async fn consumer_add(
    name: &str,
    consumer: &str,
    capabilities: Vec<String>,
    as_json: bool,
) -> Result<(), CmdError> {
    if consumer.trim().is_empty() {
        return Err(click("consumer identity must not be empty"));
    }
    let declared = capabilities.clone();
    let generation = edit_service(name, move |entry| {
        let consumers = entry
            .entry("consumers".to_string())
            .or_insert_with(|| Value::Object(Map::new()))
            .as_object_mut()
            .ok_or_else(|| click("consumers is not an object"))?;
        // An existing consumer keeps whatever else its entry carries; only the
        // declared capabilities are replaced, and only when some were given.
        let slot = consumers
            .entry(consumer.to_string())
            .or_insert_with(|| Value::Object(Map::new()));
        let slot = slot
            .as_object_mut()
            .ok_or_else(|| click(format!("consumer {consumer:?} is not an object")))?;
        if !declared.is_empty() {
            slot.insert("capabilities".to_string(), json!(declared));
        } else if !slot.contains_key("capabilities") {
            slot.insert("capabilities".to_string(), json!([]));
        }
        Ok(())
    })
    .await?;
    if as_json {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "service": name,
                "consumer": consumer,
                "capabilities": capabilities,
                "generation": generation,
            }))?
        );
    } else {
        println!("declared {consumer} on {name} generation={generation}");
    }
    Ok(())
}

async fn consumer_rm(name: &str, consumer: &str, as_json: bool) -> Result<(), CmdError> {
    let target = consumer.to_string();
    let generation = edit_service(name, move |entry| {
        let consumers = entry
            .get_mut("consumers")
            .and_then(Value::as_object_mut)
            .ok_or_else(|| click(format!("{name:?} declares no consumers")))?;
        if consumers.remove(&target).is_none() {
            let known: Vec<&str> = consumers.keys().map(String::as_str).collect();
            return Err(click(format!(
                "{name:?} does not declare {target:?}; it declares {}",
                known.join(", ")
            )));
        }
        Ok(())
    })
    .await?;
    if as_json {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "service": name,
                "removed": consumer,
                "generation": generation,
            }))?
        );
    } else {
        println!("removed {consumer} from {name} generation={generation}");
    }
    Ok(())
}
