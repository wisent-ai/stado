//! `stado delivery` — what has been written, and what has been proven.
//!
//! Writing a change and building it belong on different clocks. A session
//! finishes in minutes and there are many sessions; the fleet builds three
//! times a day. Welded together — the only recorded way to run a suite was a
//! build recipe — every session that wanted proof spent one of the fleet's
//! three builds for itself.
//!
//! This family separates them. `deliver` records a pushed revision and costs
//! nothing. `qualify` builds ONE head that carries every delivery made since
//! the last pass, runs the product's own tests once, and writes that verdict
//! onto all of them. `pending` says what is waiting, `status` says how the
//! passes went, and `failures` is the list a task register reads to reopen
//! the work that did not hold.
//!
//! Every mutation is a fenced read-modify-write of the canonical registry
//! document, the same path `stado builds` uses, so two sessions delivering at
//! the same moment cannot drop each other's record.

mod qualify;
mod records;
mod report;
mod surface;

pub use surface::{run, DeliveryCommands};

use serde_json::Value;

use crate::targets::{
    fleet_namespace_mismatch, Registry, RegistryFetchError, DELIVERIES_KEY, PASSES_KEY,
};

use super::CmdError;

/// The mutable `deliveries` array of the raw registry document, created empty
/// when the document does not carry one yet.
fn deliveries_array(document: &mut Value) -> Result<&mut Vec<Value>, CmdError> {
    array(document, DELIVERIES_KEY)
}

/// The mutable `qualification_passes` array, on the same terms.
fn passes_array(document: &mut Value) -> Result<&mut Vec<Value>, CmdError> {
    array(document, PASSES_KEY)
}

fn array<'a>(document: &'a mut Value, key: &str) -> Result<&'a mut Vec<Value>, CmdError> {
    document
        .as_object_mut()
        .ok_or_else(|| CmdError::click("registry: must be an object"))?
        .entry(key.to_string())
        .or_insert_with(|| Value::Array(Vec::new()))
        .as_array_mut()
        .ok_or_else(|| CmdError::click(format!("registry.{key}: must be an array")))
}

/// The canonical registry for read-only commands, through the same
/// last-known-good path every other CLI read uses: when the fleet object API
/// refuses or is unreachable, the answer comes from the cached copy with one
/// stderr line saying so, and the exit code stays 0.
async fn read_registry() -> Result<Registry, CmdError> {
    match crate::targets::fetch_registry_or_last_good_detail().await {
        Ok((registry, copy)) => {
            if let Some(copy) = copy {
                crate::targets::report_registry_notice(&format!(
                    "fleet store unreachable: {}; showing the registry as of {}",
                    copy.cause, copy.read_at
                ));
            }
            Ok(registry)
        }
        Err(RegistryFetchError::Absent { .. }) => Ok(Registry::default()),
        Err(error) => Err(CmdError::click(error.to_string())),
    }
}

/// The fenced registry document for a delivery mutation, refused when this
/// machine's ambient queue namespace is not the fleet's recorded one: a
/// delivery written into another namespace's registry is one the fleet never
/// qualifies.
async fn fetch_mutation_document() -> Result<(Value, String), CmdError> {
    let (document, generation) = super::registry::fetch_versioned_document().await?;
    if let Some(mismatch) = fleet_namespace_mismatch(&document) {
        return Err(CmdError::click(mismatch));
    }
    Ok((document, generation))
}

fn print_json(value: &Value) -> Result<(), CmdError> {
    println!("{}", serde_json::to_string_pretty(value)?);
    Ok(())
}
