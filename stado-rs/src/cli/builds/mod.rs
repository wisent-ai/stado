//! `stado builds` — native build recipes in the canonical registry.
//!
//! A recipe names a repository, a branch, one POSIX sh build command, the
//! artifact paths the checkout leaves behind and the release platforms it is
//! built for. The control-plane poller (`scheduler::builds`) enqueues one
//! build job PER PLATFORM whenever the branch head moves, and records the
//! outcome per platform under the recipe's `runs` map; this command family is
//! the operator surface for the recipes themselves: list, add, edit, remove,
//! enable, disable, run-now and status.
//!
//! A run's `version` is the exact git tag at the built commit (leading `v`
//! stripped) when that tag is an exact semantic version, and nothing
//! otherwise — a build of an untagged commit produces artifacts with no
//! version to declare. With `--auto-declare`, a successful run that has a
//! version writes `managed_versions` for every registry host on that
//! platform, through the same code path `stado host declare-version` uses.
//!
//! Boundary: builds publish artifacts and record versions. They never write
//! `release_control.products[...]` desired state — promoting a *signed*
//! release verifies manifests and signatures and stays the deliberate,
//! separate `stado release promote` step.
//!
//! Every mutation is a fenced read-modify-write of the canonical registry
//! document — the same raw-document read-versioned + compare-and-swap path
//! `stado host declare-version` uses — so two concurrent writers cannot
//! silently drop each other's edit. Mutations edit the raw JSON document,
//! never a re-serialized [`crate::targets::Registry`]: re-serializing the
//! typed model is a surgical key change plus a rewrite of every part it
//! never touches, and `Registry::to_document` itself warns it drops what
//! the loader drops.
//!
//! One component per seam: [`surface`] is the command surface clap parses
//! and the dispatch it feeds, [`declaration`] holds the recipe words and the
//! fenced reads and writes that declare them, [`jobs`] submits the durable
//! build jobs a run-now enqueues, and [`report`] is what `list` and `status`
//! print. The registry-document accessors all four share stay here, beside
//! the module doc that says why a mutation edits the raw document.

mod declaration;
mod jobs;
mod report;
mod surface;

pub use surface::{run, BuildsCommands};

use serde_json::Value;

use crate::targets::{
    fleet_namespace_mismatch, BuildRecipe, Registry, RegistryFetchError, BUILDS_KEY,
};

use super::CmdError;

/// The mutable `builds` array of the raw registry document, created empty
/// when the document does not carry one yet.
fn builds_array(document: &mut Value) -> Result<&mut Vec<Value>, CmdError> {
    document
        .as_object_mut()
        .ok_or_else(|| CmdError::click("registry: must be an object"))?
        .entry(BUILDS_KEY.to_string())
        .or_insert_with(|| Value::Array(Vec::new()))
        .as_array_mut()
        .ok_or_else(|| CmdError::click("registry.builds: must be an array"))
}

fn entry_name(entry: &Value) -> Option<&str> {
    entry.get("name").and_then(Value::as_str)
}

fn find_entry<'a>(entries: &'a mut [Value], name: &str) -> Result<&'a mut Value, CmdError> {
    entries
        .iter_mut()
        .find(|entry| entry_name(entry) == Some(name))
        .ok_or_else(|| CmdError::click(format!("registry declares no build recipe {name:?}")))
}

/// A raw recipe entry as the contract shape: parsed and re-serialized so
/// serde defaults fill absent fields, verbatim when it does not parse.
fn normalized_recipe_json(entry: &Value) -> Value {
    serde_json::from_value::<BuildRecipe>(entry.clone())
        .ok()
        .and_then(|recipe| serde_json::to_value(recipe).ok())
        .unwrap_or_else(|| entry.clone())
}

/// The canonical registry for read-only commands, through the same
/// last-known-good path every other CLI read uses
/// ([`crate::targets::fetch_registry_or_last_good_detail`]): when the fleet
/// object API refuses or is unreachable, the answer comes from the cached
/// copy with one stderr line saying so, and the exit code stays 0. An
/// absent document still reads as an empty registry rather than an error,
/// so `builds list` answers on a fresh deployment.
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

/// The fenced registry document for a builds MUTATION, refused when this
/// machine's ambient queue namespace is not the fleet's recorded one
/// ([`fleet_namespace_mismatch`]): a recipe written into another
/// namespace's registry is a recipe the fleet never polls, and a job
/// submitted from it is a job no fleet worker claims. Reads are exempt —
/// they degrade to the last-known-good copy instead ([`read_registry`]).
async fn fetch_mutation_document() -> Result<(Value, String), CmdError> {
    let (document, generation) = super::registry::fetch_versioned_document().await?;
    if let Some(mismatch) = fleet_namespace_mismatch(&document) {
        return Err(CmdError::click(mismatch));
    }
    Ok((document, generation))
}

fn recipe_index(recipes: &[BuildRecipe], name: &str) -> Result<usize, CmdError> {
    recipes
        .iter()
        .position(|recipe| recipe.name == name)
        .ok_or_else(|| CmdError::click(format!("registry declares no build recipe {name:?}")))
}

fn print_json(value: &Value) -> Result<(), CmdError> {
    println!("{}", serde_json::to_string_pretty(value)?);
    Ok(())
}

fn recipe_json(recipe: &BuildRecipe) -> Result<Value, CmdError> {
    Ok(serde_json::to_value(recipe)?)
}
