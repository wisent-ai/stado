//! The pins: every declaration on this host, or in the registry, that still
//! names a release version.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use serde_json::Value;

use crate::providers::local::disk_cleanup::release_store::{
    CONFIG_CANDIDATES, CONFIG_VERSION_KEY, CONFIG_VERSION_PRODUCT,
};

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
        let Ok(state) = serde_json::from_str::<serde_json::Value>(&text) else {
            continue;
        };
        let product = state
            .get("product")
            .and_then(serde_json::Value::as_str)
            .unwrap_or(stem)
            .to_string();
        let versions = pinned.entry(product).or_default();
        for slot in ["active", "previous", "candidate"] {
            if let Some(version) = state
                .get(slot)
                .and_then(|record| record.get("version"))
                .and_then(serde_json::Value::as_str)
            {
                versions.insert(version.to_string());
            }
        }
        // A quarantined digest names a version an operator may still inspect.
        if let Some(quarantined) = state
            .get("quarantined")
            .and_then(serde_json::Value::as_object)
        {
            for record in quarantined.values() {
                if let Some(version) = record.get("version").and_then(serde_json::Value::as_str) {
                    versions.insert(version.to_string());
                }
            }
        }
    }
    pinned
}

/// The versions any host in the registry DECLARES, per product, read from
/// every target's `managed_versions`.
///
/// Not just this host's target. A release version a host declares must
/// survive on whichever host carries the store, and the host that carries
/// the store is usually not the host that runs the binary: on 2026-09-04 the
/// store lived on `charless-mac-mini` while the declarations that needed
/// those bytes belonged to every other target in the fleet. Reading only the
/// local target's declaration would leave that gap exactly as it was.
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn set(names: &[&str]) -> BTreeSet<String> {
        names.iter().map(|name| name.to_string()).collect()
    }

    fn write(path: &Path, bytes: &[u8]) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, bytes).unwrap();
    }

    #[test]
    fn every_target_contributes_its_declared_versions() {
        let registry = json!({
            "targets": [
                {"name": "mini", "managed_versions": {"stado": "0.15.21", "skarbiec": "0.4.0"}},
                {"name": "macbook", "managed_versions": {"stado": "0.16.1"}},
                {"name": "blank", "managed_versions": {"stado": "   "}},
                {"name": "wrong-type", "managed_versions": {"stado":
                    15}},
                {"name": "undeclared"}
            ]
        });
        let declared = declared_versions(&registry);
        assert_eq!(declared["stado"], set(&["0.15.21", "0.16.1"]));
        assert_eq!(declared["skarbiec"], set(&["0.4.0"]));
    }

    #[test]
    fn a_registry_without_targets_pins_nothing_and_does_not_fail() {
        assert!(declared_versions(&json!({})).is_empty());
        assert!(declared_versions(&json!({"targets": "not an array"})).is_empty());
    }

    #[test]
    fn the_version_pin_is_read_from_every_config_candidate() {
        let home = tempfile::tempdir().unwrap();
        write(
            &home.path().join(".config/stado/config.json"),
            br#"{"release": {"version": "0.15.3"}}"#,
        );
        // A file the resolver would never reach because the one above wins.
        // Its pin is still a version somebody wrote down on this host.
        write(
            &home.path().join(".stado/config.json"),
            br#"{"release": {"version": "0.14.6"}}"#,
        );
        write(&home.path().join("stado.config.json"), b"{ not json");
        let pinned = config_pinned_versions(home.path());
        assert_eq!(pinned[CONFIG_VERSION_PRODUCT], set(&["0.14.6", "0.15.3"]));
    }

    #[test]
    fn a_home_with_no_config_pins_nothing() {
        let home = tempfile::tempdir().unwrap();
        assert!(config_pinned_versions(home.path()).is_empty());
        write(
            &home.path().join(".stado/config.json"),
            br#"{"release": {"platform": "darwin-arm64"}}"#,
        );
        assert!(config_pinned_versions(home.path()).is_empty());
    }
}
