//! The pins: every declaration on this host, or in the registry, that still
//! names a release version.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use serde_json::Value;

use crate::providers::local::disk_cleanup::release_store::{
    CONFIG_CANDIDATES, CONFIG_VERSION_KEY, CONFIG_VERSION_PRODUCT,
};

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
/// product, read from every state file in `state_dir`.
pub(in crate::providers::local::disk_cleanup::release_store) fn host_pinned_versions(
    state_dir: &Path,
) -> BTreeMap<String, BTreeSet<String>> {
    let mut pinned: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let Ok(entries) = std::fs::read_dir(state_dir) else {
        return pinned;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        if path.extension().and_then(|e| e.to_str()) != Some("json") || stem.ends_with("-proxy") {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Ok(state) = serde_json::from_str::<PinnedSlots>(&text) else {
            continue;
        };
        let product = state.product.clone().map_or(stem.to_string(), |name| name);
        let versions = pinned.entry(product).or_default();
        for slot in [state.active, state.previous, state.candidate]
            .into_iter()
            .flatten()
        {
            versions.insert(slot.version);
        }
    }
    pinned
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
/// what this reads.
pub(in crate::providers::local::disk_cleanup::release_store) fn config_pinned_versions(
    home: &Path,
) -> BTreeMap<String, BTreeSet<String>> {
    let mut pinned: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for candidate in CONFIG_CANDIDATES {
        let path = home.join(candidate);
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Ok(document) = serde_json::from_str::<Value>(&text) else {
            continue;
        };
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
        let Some(version) = node.as_str().map(str::trim).filter(|v| !v.is_empty()) else {
            continue;
        };
        pinned
            .entry(CONFIG_VERSION_PRODUCT.to_string())
            .or_default()
            .insert(version.to_string());
    }
    pinned
}
