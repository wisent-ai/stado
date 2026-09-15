//! `stado service directory consumer-add` and `... consumer-rm` — who may use
//! a service, and the conditional write that records the change.

use serde_json::{json, Map, Value};

use crate::cli::registry;
use crate::cli::CmdError;

use crate::cli::directory::document::{click, directory, service, DIRECTORY_KEY};

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
        next_generation
            .set(crate::service_resolution::advance_generation(&mut document).map_err(click)?);
        Ok(document)
    })
    .await?;
    Ok(next_generation.get())
}

fn consumer_entry<'a>(document: &'a mut Value, name: &str) -> Result<&'a mut Map<String, Value>, CmdError> {
    document
        .get_mut(DIRECTORY_KEY)
        .and_then(Value::as_object_mut)
        .and_then(|block| block.get_mut("services"))
        .and_then(Value::as_object_mut)
        .and_then(|all| all.get_mut(name))
        .and_then(Value::as_object_mut)
        .ok_or_else(|| click(format!("service {name:?} is not an object")))
}

fn bind_consumer(
    document: &mut Value,
    service: &str,
    consumer: &str,
    target: &str,
    bind: std::net::SocketAddr,
) -> Result<(), CmdError> {
    if !bind.ip().is_loopback() || bind.port() == 0 {
        return Err(click("consumer binding requires a loopback IP and a nonzero port"));
    }
    let target_entry = document
        .get_mut("targets")
        .and_then(Value::as_array_mut)
        .and_then(|targets| targets.iter_mut().find(|entry| entry["name"] == target))
        .ok_or_else(|| click(format!("resolver target {target:?} is not registered")))?;
    let adapters = target_entry
        .get_mut("service_resolver")
        .and_then(|config| config.get_mut("adapters"))
        .and_then(Value::as_array_mut)
        .ok_or_else(|| click(format!("resolver target {target:?} has no configured adapters")))?;
    let mut existing = None;
    for (index, adapter) in adapters.iter().enumerate() {
        if adapter["service"] == service && adapter["consumer"] == consumer
            && existing.replace(index).is_some()
        {
            return Err(click(format!("resolver target {target:?} has ambiguous bindings for {service}/{consumer}")));
        }
    }
    if let Some(index) = existing {
        adapters[index]["bind"] = json!(bind.to_string());
    } else {
        adapters.push(json!({"service": service, "consumer": consumer, "bind": bind.to_string()}));
    }
    Ok(())
}

pub(in crate::cli::directory) async fn consumer_add(
    name: &str,
    consumer: &str,
    capabilities: Vec<String>,
    binding: Option<(String, std::net::SocketAddr)>,
    as_json: bool,
) -> Result<(), CmdError> {
    if consumer.trim().is_empty() {
        return Err(click("consumer identity must not be empty"));
    }
    let declared = capabilities.clone();
    let generation = edit_service(name, |document| {
        let entry = consumer_entry(document, name)?;
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
        if let Some((target, bind)) = &binding {
            bind_consumer(document, name, consumer, target, *bind)?;
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
                "binding": binding.as_ref().map(|(target, bind)| json!({"target": target, "bind": bind.to_string()})),
            }))?
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
            .ok_or_else(|| click(format!("{name:?} declares no consumers")))?;
        if consumers.remove(consumer).is_none() {
            let known: Vec<&str> = consumers.keys().map(String::as_str).collect();
            return Err(click(format!(
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
                    adapters.retain(|adapter| adapter["service"] != name || adapter["consumer"] != consumer);
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
