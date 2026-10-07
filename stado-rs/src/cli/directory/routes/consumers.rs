//! `stado service directory consumer-add` and `... consumer-rm` — who may use
//! a service, and the conditional write that records the change.

use serde_json::{json, Map, Value};

use crate::cli::registry;
use crate::cli::CmdError;

use crate::cli::directory::document::{directory, service, DIRECTORY_KEY};

/// Commit the consumer policy and dependent resolver bindings together.
///
/// The transform preserves unrelated fields, validates the complete result,
/// and runs again against the latest document after a competing writer wins.
///
/// `advance_generation` runs INSIDE the transform because it derives the next
/// counter from the document it was handed. Computing it against the first
/// read and reusing it after a retry would republish a number the newer
/// document has already passed — the same reverted-directory bug the counter
/// exists to make visible. The generation returned is the one the round that
/// actually landed produced.
async fn edit_service<F>(name: &str, edit: F) -> Result<u64, CmdError>
where
    F: Fn(&mut Value) -> Result<(), CmdError>,
{
    let next_generation = std::cell::Cell::new(0);
    registry::commit_document(|current| {
        let mut document = current.clone();
        {
            let block = directory(&document)?;
            service(block, name)?;
        }
        edit(&mut document)?;
        next_generation.set(
            crate::service_resolution::advance_generation(&mut document)
                .map_err(CmdError::declaration)?,
        );
        Ok(document)
    })
    .await?;
    Ok(next_generation.get())
}

fn consumer_entry<'a>(
    document: &'a mut Value,
    name: &str,
) -> Result<&'a mut Map<String, Value>, CmdError> {
    document
        .get_mut(DIRECTORY_KEY)
        .and_then(Value::as_object_mut)
        .and_then(|block| block.get_mut("services"))
        .and_then(Value::as_object_mut)
        .and_then(|all| all.get_mut(name))
        .and_then(Value::as_object_mut)
        .ok_or_else(|| CmdError::declaration(format!("service {name:?} is not an object")))
}

/// Record `consumer`'s route to `service` on `target` and answer the address
/// the directory now holds for it.
///
/// An adapter the target already declares keeps its address, so a repeated
/// declaration never moves a port under a running client. A new one takes
/// `offered`, the port the target's own system handed out: nobody chooses
/// the number, and the directory is where every reader looks it up.
fn bind_consumer(
    document: &mut Value,
    service: &str,
    consumer: &str,
    target: &str,
    offered: u16,
) -> Result<String, CmdError> {
    let target_entry = document
        .get_mut("targets")
        .and_then(Value::as_array_mut)
        .and_then(|targets| targets.iter_mut().find(|entry| entry["name"] == target))
        .ok_or_else(|| {
            CmdError::missing(format!("resolver target {target:?} is not registered"))
        })?;
    let adapters = target_entry
        .get_mut("service_resolver")
        .and_then(|config| config.get_mut("adapters"))
        .and_then(Value::as_array_mut)
        .ok_or_else(|| {
            CmdError::declaration(format!(
                "resolver target {target:?} has no configured adapters"
            ))
        })?;
    let mut existing = None;
    for (index, adapter) in adapters.iter().enumerate() {
        if adapter["service"] == service
            && adapter["consumer"] == consumer
            && existing.replace(index).is_some()
        {
            return Err(CmdError::declaration(format!(
                "resolver target {target:?} has ambiguous bindings for {service}/{consumer}"
            )));
        }
    }
    if let Some(index) = existing {
        return adapters[index]["bind"]
            .as_str()
            .map(str::to_string)
            .ok_or_else(|| {
                CmdError::declaration(format!(
                    "resolver target {target:?} declares {service}/{consumer} without an address"
                ))
            });
    }
    let bind = std::net::SocketAddr::from((std::net::Ipv4Addr::LOCALHOST, offered)).to_string();
    // The host hands out a port that is free right now; an adapter declared
    // earlier and not yet bound by the resolver may already hold it on paper.
    if let Some(holder) = adapters
        .iter()
        .find(|adapter| adapter["bind"] == bind.as_str())
    {
        return Err(CmdError::refused(format!(
            "{target} handed out {bind}, which its {}/{} adapter already declares and the \
             resolver has not bound yet; run the declaration again for another port",
            holder["service"].as_str().unwrap_or_default(),
            holder["consumer"].as_str().unwrap_or_default()
        )));
    }
    adapters.push(json!({"service": service, "consumer": consumer, "bind": bind}));
    Ok(bind)
}

pub(in crate::cli::directory) async fn consumer_add(
    name: &str,
    consumer: &str,
    capabilities: Vec<String>,
    target: Option<String>,
    as_json: bool,
) -> Result<(), CmdError> {
    if consumer.trim().is_empty() {
        return Err(CmdError::usage("consumer identity must not be empty"));
    }
    // Asked before the write, because the transform below is synchronous and
    // may run again after a competing writer: the host hands out one port,
    // and only a consumer the target does not route yet takes it.
    let offered = match &target {
        Some(target) => {
            let host = crate::cli::canonical_host(target).await?;
            Some((
                host.name.clone(),
                crate::cli::directory::routes::assigned::host_free_port(
                    &host,
                    &crate::deploy::production_runner(),
                )
                .await?,
            ))
        }
        None => None,
    };
    let recorded = std::cell::RefCell::new(None::<String>);
    let declared = capabilities.clone();
    let generation = edit_service(name, |document| {
        let entry = consumer_entry(document, name)?;
        let consumers = entry
            .entry("consumers".to_string())
            .or_insert_with(|| Value::Object(Map::new()))
            .as_object_mut()
            .ok_or_else(|| CmdError::declaration("consumers is not an object"))?;
        // An existing consumer keeps whatever else its entry carries; only the
        // declared capabilities are replaced, and only when some were given.
        let slot = consumers
            .entry(consumer.to_string())
            .or_insert_with(|| Value::Object(Map::new()));
        let slot = slot.as_object_mut().ok_or_else(|| {
            CmdError::declaration(format!("consumer {consumer:?} is not an object"))
        })?;
        if !declared.is_empty() {
            slot.insert("capabilities".to_string(), json!(declared));
        } else if !slot.contains_key("capabilities") {
            slot.insert("capabilities".to_string(), json!([]));
        }
        if let Some((target, port)) = &offered {
            recorded.replace(Some(bind_consumer(
                document, name, consumer, target, *port,
            )?));
        }
        Ok(())
    })
    .await?;
    let binding = offered
        .as_ref()
        .zip(recorded.into_inner())
        .map(|((target, _), bind)| (target.clone(), bind));
    if as_json {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "service": name,
                "consumer": consumer,
                "capabilities": capabilities,
                "generation": generation,
                "binding": binding.as_ref().map(|(target, bind)| json!({"target": target, "bind": bind})),
            }))?
        );
    } else if let Some((target, bind)) = &binding {
        println!(
            "declared {consumer} on {name} generation={generation}; {target} routes it at {bind}"
        );
    } else {
        println!("declared {consumer} on {name} generation={generation}");
    }
    Ok(())
}

pub(in crate::cli::directory) async fn consumer_rm(
    name: &str,
    consumer: &str,
    as_json: bool,
) -> Result<(), CmdError> {
    let removed_bindings = std::cell::Cell::new(0);
    let generation = edit_service(name, |document| {
        let entry = consumer_entry(document, name)?;
        let consumers = entry
            .get_mut("consumers")
            .and_then(Value::as_object_mut)
            .ok_or_else(|| CmdError::missing(format!("{name:?} declares no consumers")))?;
        if consumers.remove(consumer).is_none() {
            let known: Vec<&str> = consumers.keys().map(String::as_str).collect();
            return Err(CmdError::missing(format!(
                "{name:?} does not declare {consumer:?}; it declares {}",
                known.join(", ")
            )));
        }
        let mut removed = 0;
        if let Some(targets) = document.get_mut("targets").and_then(Value::as_array_mut) {
            for target in targets {
                if let Some(adapters) = target
                    .get_mut("service_resolver")
                    .and_then(|config| config.get_mut("adapters"))
                    .and_then(Value::as_array_mut)
                {
                    let previous = adapters.len();
                    adapters.retain(|adapter| {
                        adapter["service"] != name || adapter["consumer"] != consumer
                    });
                    removed += previous - adapters.len();
                }
            }
        }
        removed_bindings.set(removed);
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
                "bindings_removed": removed_bindings.get(),
            }))?
        );
    } else {
        println!("removed {consumer} from {name} generation={generation}");
    }
    Ok(())
}
