//! The coordinator section of a registry document, and the contracts the
//! other products in this binary own.
//!
//! Split out of `document/registry.rs`, which had grown past the module line
//! cap; the target section stays there and calls both of these.

use std::collections::{HashMap, HashSet};

use crate::targets::*;

/// A coordinator names either a host or a heuristic that selects one, never
/// both; the heuristic must match a local target that exists, and two
/// coordinators may not select through the same one.
pub(super) fn validate_coordinators(
    root: &serde_json::Map<String, Value>,
    target_heuristics: &HashMap<&str, &str>,
) -> Result<(), RegistryValidationError> {
    let Some(coordinators) = root.get("coordinators") else {
        return Ok(());
    };
    let coordinators = coordinators
        .as_array()
        .ok_or_else(|| verr("registry.coordinators", "must be an array"))?;
    let mut coordinator_heuristics: HashSet<&str> = HashSet::new();
    for (index, coordinator) in coordinators.iter().enumerate() {
        let location = format!("registry.coordinators[{index}]");
        let coordinator = coordinator
            .as_object()
            .ok_or_else(|| verr(&location, "must be an object"))?;
        let Some(heuristic) = coordinator.get("host_heuristic") else {
            continue;
        };
        let heuristic = heuristic
            .as_str()
            .ok_or_else(|| verr(&format!("{location}.host_heuristic"), "must be a string"))?;
        if coordinator.get("host").is_some_and(|host| !host.is_null()) {
            return Err(verr(
                &location,
                "must not declare both host and host_heuristic",
            ));
        }
        if !target_heuristics.contains_key(heuristic) {
            return Err(verr(
                &format!("{location}.host_heuristic"),
                &format!("matches no local target: '{heuristic}'"),
            ));
        }
        if !coordinator_heuristics.insert(heuristic) {
            return Err(verr(
                &format!("{location}.host_heuristic"),
                &format!("selector '{heuristic}' is already used by another coordinator"),
            ));
        }
    }
    Ok(())
}

/// Each product judges its own part of the same document.
///
/// `include_inference` is false only for the write path, which re-runs that
/// section itself so it can scope a failure to writes that actually touch it.
pub(super) fn validate_product_contracts(
    data: &Value,
    include_inference: bool,
) -> Result<(), RegistryValidationError> {
    crate::placement::validate_registry_contract(data).map_err(RegistryValidationError)?;
    crate::service_resolution::validate_registry_contract(data).map_err(RegistryValidationError)?;
    crate::release_control::validate_registry_contract(data).map_err(RegistryValidationError)?;
    // The unit-image revisit policy is not judged here. It names launchd
    // labels, and a label the product catalog renames (every product moved
    // to com.wisent.<product> at once) turns a block that was valid when it
    // was written into a refused one the moment a build carrying the rename
    // reads it: every command on every host stopped reading the registry.
    // Its readers — the release agent's revisit pass and the
    // stale-unit-image annotations — validate it themselves and report a
    // refusal on their own line, and a write that changes the block is held
    // to the full check by `validate_registry_for_write`.
    //
    // The public-origin block is judged here so an operator learns at the
    // write: an origin
    // nothing outside the tailnet can resolve reached the public release
    // route through an untyped deployment variable, and no reader refused it.
    crate::public_origin::validate_registry_contract(data).map_err(RegistryValidationError)?;

    if include_inference {
        crate::inference::schema::validate(data).map_err(RegistryValidationError)?;
    }
    Ok(())
}
