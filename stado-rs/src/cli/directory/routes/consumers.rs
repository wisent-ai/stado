//! `stado service directory consumer-add` and `... consumer-rm` — who may use
//! a service, and the conditional write that records the change.

use serde_json::{json, Map, Value};

use crate::cli::registry;
use crate::cli::CmdError;

use crate::cli::directory::document::{click, directory, service, DIRECTORY_KEY};

/// Mutate one service entry in place and write the whole document back
/// conditionally on the generation it was read at.
///
/// The closure sees the service's own object, so nothing outside it can be
/// touched, and the write goes through `commit_document`, which validates the
/// document, refuses one that would delete a top-level key, and re-reads and
/// re-applies `edit` when another writer published first. `edit` is therefore
/// `Fn`, not `FnOnce`: it may run once per round.
///
/// `advance_generation` runs INSIDE the transform because it derives the next
/// counter from the document it was handed. Computing it against the first
/// read and reusing it after a retry would republish a number the newer
/// document has already passed — the same reverted-directory bug the counter
/// exists to make visible. The generation returned is the one the round that
/// actually landed produced.
async fn edit_service<F>(name: &str, edit: F) -> Result<u64, CmdError>
where
    F: Fn(&mut Map<String, Value>) -> Result<(), CmdError>,
{
    let next_generation = std::cell::Cell::new(0);
    registry::commit_document(|current| {
        let mut document = current.clone();
        {
            let block = directory(&document)?;
            service(block, name)?;
        }
        {
            let entry = document
                .get_mut(DIRECTORY_KEY)
                .and_then(Value::as_object_mut)
                .and_then(|block| block.get_mut("services"))
                .and_then(Value::as_object_mut)
                .and_then(|all| all.get_mut(name))
                .and_then(Value::as_object_mut)
                .ok_or_else(|| click(format!("service {name:?} is not an object")))?;
            edit(entry)?;
        }
        next_generation
            .set(crate::service_resolution::advance_generation(&mut document).map_err(click)?);
        Ok(document)
    })
    .await?;
    Ok(next_generation.get())
}

pub(in crate::cli::directory) async fn consumer_add(
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

pub(in crate::cli::directory) async fn consumer_rm(
    name: &str,
    consumer: &str,
    as_json: bool,
) -> Result<(), CmdError> {
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
