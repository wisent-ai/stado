//! The pins: every declaration on this host, or in the registry, that still
//! names a release version.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use serde_json::Value;

use crate::providers::local::space_reclaim::release_store::{
    CONFIG_CANDIDATES, CONFIG_VERSION_KEY, CONFIG_VERSION_PRODUCT,
};
use crate::providers::local::space_reclaim::JanitorError;

/// A pin source this cleaner cannot read: the versions it pins are unknown,
/// and a pin this cleaner cannot see is a version it would delete, so the
/// pass reclaims nothing.
fn unknown(path: &Path, cause: String) -> JanitorError {
    JanitorError::os(&format!(
        "{} {cause}, so the release versions it pins are unknown and nothing is reclaimed",
        path.display()
    ))
}

/// The text of `path`, `None` when it is not there.
fn pin_text(path: &Path) -> Result<Option<String>, JanitorError> {
    match std::fs::read_to_string(path) {
        Ok(text) => Ok(Some(text)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(unknown(path, format!("cannot be read: {error}"))),
    }
}

/// The slots of a release agent state document
/// ([`crate::release_agent::HostReleaseState`]) that name a version, read
/// without its other fields so a non-release file in the same directory (the
/// unit-image ledger) reads as pinning nothing rather than failing.
#[derive(serde::Deserialize)]
struct PinnedSlots {
    #[serde(default)]
    product: Option<String>,
    #[serde(default)]
    active: Option<Slot>,
    #[serde(default)]
    previous: Option<Slot>,
    #[serde(default)]
    candidate: Option<Slot>,
}

#[derive(serde::Deserialize)]
struct Slot {
    version: String,
}

/// The versions the release agent on this host still has a use for, per
/// product, read from every state file in `state_dir`. A state directory
/// that is not there pins nothing; a file that cannot be read, or whose
/// slots do not parse, is the error naming it.
pub(in crate::providers::local::space_reclaim::release_store) fn host_pinned_versions(
    state_dir: &Path,
) -> Result<BTreeMap<String, BTreeSet<String>>, JanitorError> {
    let mut pinned: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let entries = match std::fs::read_dir(state_dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(pinned),
        Err(error) => return Err(unknown(state_dir, format!("cannot be listed: {error}"))),
    };
    for entry in entries {
        let path = entry
            .map_err(|error| unknown(state_dir, format!("cannot be listed: {error}")))?
            .path();
        let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        if path.extension().and_then(|e| e.to_str()) != Some("json") || stem.ends_with("-proxy") {
            continue;
        }
        let Some(text) = pin_text(&path)? else {
            continue;
        };
        let state = serde_json::from_str::<PinnedSlots>(&text)
            .map_err(|error| unknown(&path, format!("does not parse as release slots: {error}")))?;
        let product = state.product.clone().map_or(stem.to_string(), |name| name);
        let versions = pinned.entry(product).or_default();
        for slot in [state.active, state.previous, state.candidate]
            .into_iter()
            .flatten()
        {
            versions.insert(slot.version);
        }
    }
    Ok(pinned)
}

/// The versions any host in the registry DECLARES, per product, read from
/// every target's `managed_versions`.
///
/// Not just this host's target. A release version a host declares must
/// survive on whichever host carries the store, and the host that carries
/// the store is usually not the host that runs the binary: the declarations
/// that need those bytes belong to every other target in the fleet. Reading
/// only the local target's declaration would leave that gap open.
///
/// The registry document is taken as `Value` rather than as parsed targets
/// because this cleaner must not fail closed on a target shape a newer
/// release added: an unreadable target contributes no pin, and a pin this
/// cleaner cannot see is a version it would delete.
pub fn declared_versions(registry: &Value) -> BTreeMap<String, BTreeSet<String>> {
    let mut pinned: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let Some(targets) = registry.get("targets").and_then(Value::as_array) else {
        return pinned;
    };
    for target in targets {
        let Some(declared) = target
            .get(crate::deploy::host_release::MANAGED_VERSIONS_KEY)
            .and_then(Value::as_object)
        else {
            continue;
        };
        for (product, version) in declared {
            let Some(version) = version.as_str() else {
                continue;
            };
            let version = version.trim();
            if version.is_empty() {
                continue;
            }
            pinned
                .entry(product.clone())
                .or_default()
                .insert(version.to_string());
        }
    }
    pinned
}

/// The versions an operator pins in a config file on this host, per product,
/// from `release.version` in every candidate config path under `home`.
///
/// This is the pin `install-stado.sh` reads as `STADO_RELEASE_VERSION`, the
/// declared release host-state capability reconciles, and
/// `providers/local/version_check.rs` measures a host against. An environment
/// variable cannot be a pin here — it belongs to whichever process exported
/// it, not to the host — so the file is the durable half, and the file is
/// what this reads. A config that is not there, or names no version, pins
/// nothing; one that cannot be read or parsed, or whose version is not text,
/// is the error naming it.
pub(in crate::providers::local::space_reclaim::release_store) fn config_pinned_versions(
    home: &Path,
) -> Result<BTreeMap<String, BTreeSet<String>>, JanitorError> {
    let mut pinned: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for candidate in CONFIG_CANDIDATES {
        let path = home.join(candidate);
        let Some(text) = pin_text(&path)? else {
            continue;
        };
        let document = serde_json::from_str::<Value>(&text)
            .map_err(|error| unknown(&path, format!("is not JSON: {error}")))?;
        let mut node = &document;
        for key in CONFIG_VERSION_KEY {
            match node.get(key) {
                Some(next) => node = next,
                None => {
                    node = &Value::Null;
                    break;
                }
            }
        }
        let version = match node {
            Value::Null => continue,
            Value::String(version) => version.trim(),
            other => {
                return Err(unknown(
                    &path,
                    format!("names release.version {other}, which is not text"),
                ))
            }
        };
        if version.is_empty() {
            continue;
        }
        pinned
            .entry(CONFIG_VERSION_PRODUCT.to_string())
            .or_default()
            .insert(version.to_string());
    }
    Ok(pinned)
}
