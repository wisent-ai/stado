//! The stores the catalog declares for the products a placement profile
//! runs (`service.state`), the ones a profile does not carry, and
//! `stado placement profile sync`, which adds those.
//!
//! A profile declared before the catalog named a product's stores, or
//! written by hand, moves the units and leaves what the services keep on the
//! fenced source; Brama's donated subscriptions are one such store. A move
//! refuses such a profile, naming `sync`, which gives it what the
//! catalog declares; `placement evacuate --standby` runs the same pass
//! ([`add_missing`]) in its run, and its check lists what it would add.

use serde_json::{json, Value};

use crate::cli::placement::candidates::{ensure_profile_lifecycle_mutable, unit};
use crate::cli::{registry, CmdError};
use crate::deploy::service_catalog::{lookup, CatalogService, CatalogState};
use crate::placement::{self, PlacementHost, PlacementProfile};

/// The catalog entry of the product a service runs, by the first of `names`
/// the catalog knows: the service's own name, then the unit's recorded
/// name, then the label it is installed as.
pub(in crate::cli::placement) fn catalog_product(
    names: &[&str],
) -> Result<Option<CatalogService>, CmdError> {
    names
        .iter()
        .map(|name| lookup(name))
        .find_map(Result::transpose)
        .transpose()
        .map_err(CmdError::declaration)
}

/// Every store the catalog declares for a product of `profile`, found
/// through the units `host` holds, each path once, with the product that
/// keeps it, which `profile` does not carry among its states.
pub(in crate::cli::placement) fn missing_stores(
    profile: &PlacementProfile,
    host: &PlacementHost,
) -> Result<Vec<(String, CatalogState)>, CmdError> {
    let mut missing: Vec<(String, CatalogState)> = Vec::new();
    for logical in &profile.services {
        let placed = unit(host, logical)?;
        let mut names = vec![logical.as_str(), placed.name.as_str()];
        names.extend(placed.managed().map(|managed| managed.unit.as_str()));
        let Some(entry) = catalog_product(&names)? else {
            continue;
        };
        for store in entry.state {
            let carried = profile.state.iter().any(|state| state.path == store.path);
            let listed = missing.iter().any(|(_, kept)| kept.path == store.path);
            if !carried && !listed {
                missing.push((entry.name.clone(), store));
            }
        }
    }
    Ok(missing)
}

/// The stores `profile` lacks, read through any one of its hosts: every host
/// of a profile holds the same services.
pub(crate) fn lacking(profile: &PlacementProfile) -> Result<Vec<(String, CatalogState)>, CmdError> {
    let host = profile.hosts.values().next().ok_or_else(|| {
        CmdError::declaration(format!("placement profile {} names no host", profile.name))
    })?;
    missing_stores(profile, host)
}

/// The words a refusal or a receipt lists missing stores in.
pub(crate) fn listed(missing: &[(String, CatalogState)]) -> String {
    missing
        .iter()
        .map(|(product, store)| format!("{} ({product})", store.path))
        .collect::<Vec<_>>()
        .join(", ")
}

/// What one sync added, and the registry generation that holds it; nothing
/// and no generation when the profile carried every store already.
pub(in crate::cli::placement) struct Synced {
    pub added: Vec<(String, CatalogState)>,
    pub generation: Option<String>,
}

/// Add to profile `name` every store the catalog declares for its products
/// that it does not carry, in one compare-and-swapped commit. An unknown
/// profile and one a move holds are refused.
pub(in crate::cli::placement) async fn add_missing(name: &str) -> Result<Synced, CmdError> {
    let document = registry::fetch_document().await?;
    let profile = placement::profiles(&document)
        .map_err(CmdError::declaration)?
        .into_iter()
        .find(|profile| profile.name == name)
        .ok_or_else(|| {
            CmdError::missing(format!(
                "the registry declares no placement profile named {name:?}"
            ))
        })?;
    ensure_profile_lifecycle_mutable(&profile)?;
    let missing = lacking(&profile)?;
    if missing.is_empty() {
        return Ok(Synced {
            added: missing,
            generation: None,
        });
    }
    let entries: Vec<Value> = missing
        .iter()
        .map(|(_, store)| json!({ "path": store.path, "required": store.required, "tree": store.tree }))
        .collect();
    let generation = registry::commit_document(|current| {
        let mut next = current.clone();
        let entry = next
            .get_mut("placement_profiles")
            .and_then(Value::as_array_mut)
            .and_then(|profiles| {
                profiles
                    .iter_mut()
                    .find(|entry| entry.get("name").and_then(Value::as_str) == Some(name))
            })
            .ok_or_else(|| CmdError::missing(format!("placement profile {name} disappeared")))?;
        let state = entry
            .as_object_mut()
            .ok_or_else(|| {
                CmdError::declaration(format!("placement profile {name} is not an object"))
            })?
            .entry("state")
            .or_insert_with(|| Value::Array(Vec::new()))
            .as_array_mut()
            .ok_or_else(|| {
                CmdError::declaration(format!("placement profile {name} state is not a list"))
            })?;
        for store in &entries {
            if !state
                .iter()
                .any(|kept| kept.get("path") == store.get("path"))
            {
                state.push(store.clone());
            }
        }
        Ok(next)
    })
    .await?;
    Ok(Synced {
        added: missing,
        generation: Some(generation),
    })
}

/// `stado placement profile sync NAME [--json]`: [`add_missing`], said.
pub(super) async fn sync_state(name: &str, json_output: bool) -> Result<(), CmdError> {
    let Synced { added, generation } = add_missing(name).await?;
    let entries: Vec<Value> = added
        .iter()
        .map(|(_, store)| json!({ "path": store.path, "required": store.required, "tree": store.tree }))
        .collect();
    match (&generation, json_output) {
        (None, true) => {
            let answer = json!({ "profile": name, "state": "unchanged", "added": entries });
            println!("{}", serde_json::to_string_pretty(&answer)?);
        }
        (None, false) => {
            println!("placement profile {name} carries every store the catalog declares for its products");
        }
        (Some(generation), true) => {
            let answer = json!({
                "profile": name,
                "state": "synced",
                "added": entries,
                "registry_generation": generation,
            });
            println!("{}", serde_json::to_string_pretty(&answer)?);
        }
        (Some(generation), false) => println!(
            "placement profile {name} now carries {} (registry generation {generation})",
            listed(&added)
        ),
    }
    Ok(())
}

/// `stado placement profile sync --all [--json]`: [`add_missing`] for
/// every profile the registry declares, one commit each, so the gap
/// `registry doctor` names as `profile-lacks-catalog-stores` closes in one
/// command. A profile a move holds, or whose units the catalog cannot be
/// read against, is named with its cause and the others still sync; the
/// command then refuses, naming them.
pub(super) async fn sync_every_state(json_output: bool) -> Result<(), CmdError> {
    let document = registry::fetch_document().await?;
    let names: Vec<String> = placement::profiles(&document)
        .map_err(CmdError::declaration)?
        .into_iter()
        .map(|profile| profile.name)
        .collect();
    let mut rows = Vec::new();
    let mut refused = Vec::new();
    for name in &names {
        match add_missing(name).await {
            Ok(Synced { added, generation }) => {
                if !json_output {
                    match &generation {
                        Some(generation) => println!(
                            "placement profile {name} now carries {} (registry generation {generation})",
                            listed(&added)
                        ),
                        None => println!("placement profile {name} carries every store the catalog declares"),
                    }
                }
                rows.push(json!({
                    "profile": name,
                    "state": if generation.is_some() { "synced" } else { "unchanged" },
                    "added": added.iter().map(|(_, store)| store.path.clone()).collect::<Vec<_>>(),
                    "registry_generation": generation,
                }));
            }
            Err(error) => {
                if !json_output {
                    println!("placement profile {name} was not synced: {error}");
                }
                rows.push(
                    json!({ "profile": name, "state": "refused", "refused": error.to_string() }),
                );
                refused.push(name.clone());
            }
        }
    }
    if json_output {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({ "profiles": rows }))?
        );
    }
    if refused.is_empty() {
        return Ok(());
    }
    Err(CmdError::refused(format!(
        "every other profile was synced, but {} could not be: each line above names why",
        refused.join(", ")
    )))
}
