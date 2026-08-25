//! Compute-target registry: data models, hostname validation, capability
//! admission, and local-file loading.
//!
//! Port of `stado/targets/__init__.py` (dataclasses + loader),
//! `stado/targets/validation.py` (registry-v2 contract + hostname
//! normalization), and `stado/targets/capabilities.py` (workload admission
//! against declared target capabilities — pure logic over the [`Job`]
//! model).
//!
//! The registry is the single source of truth for every box the queue can
//! route to: workstations, GCP zonal dispatchers, vast.ai pools. Like
//! Python, [`fetch_registry_remote`] fetches `registry.json` with a
//! short-TTL in-process cache and is the fleet-survival authority
//! (`source="gcs"`), while [`load_registry_auto`] adds the bundled file as
//! a fallback (`source="auto"`). On the "gcs" backend the fetch still goes
//! through the crate's GCS JSON-API backend, never gsutil — see the Python
//! `_load_from_gcs` docstring: a broken gsutil install knocked the agent
//! offline on 2026-05-08 even though the registry was in GCS.
//!
//! The same document carries the fleet's [`ServiceDirectory`] — which host
//! currently serves each service and which consumers may call it — and the
//! [`PlacementProfile`] groups that move those services between hosts.
//! [`Registry`] keeps every top-level key it does not model in `extra`, so a
//! writer built from this checkout cannot delete a block a newer publisher
//! added.
//!
//! DEVIATION from Python: the fetch follows `WC_STORAGE_BACKEND` instead of
//! hardcoding GCS. Python reads GCS unconditionally, so on an Azure-only
//! deployment the write side compare-and-swaps `registry.json` into the
//! Azure container (`cli::registry` — `stado registry push`, which already
//! goes through the configured store) while every reader consults a GCS
//! object nobody writes. The "gcs" read path is unchanged.
//!
//! [`fetch_registry_remote`] returns [`RegistryFetchError`] rather than an
//! empty registry, because "the store is unreachable" and "the registry
//! does not list you" drive opposite decisions in the coordinator's
//! rogue-daemon kill switch.
//!
//! A reader is not required to die with the authority. Every canonical read
//! that parses is copied to `~/.stado/cache/registry-last-good.json` with a
//! dated sidecar, and [`fetch_registry_or_last_good`] serves that copy —
//! carrying its age in [`Registry::staleness_seconds`] and one sentence for
//! the operator — when the store does not answer. The bundled snapshot stays
//! BELOW the cache, reachable only through [`load_registry_auto`]. What does
//! not change is the kill switch's authority: [`fetch_registry_remote`] still
//! fails rather than answer from a copy, because "the registry no longer
//! lists you" may only be concluded from the registry itself.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

use chrono::{DateTime, SecondsFormat, Utc};
use futures::future::BoxFuture;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::models::Job;
use crate::queue::{BlobBackend, JobStorage, StorageError, VersionedText};

// ---------------------------------------------------------------------------
// validation.py — hostname normalization
// ---------------------------------------------------------------------------

/// Return the canonical form used for host identity comparisons.
pub fn normalize_hostname(value: &str) -> String {
    value
        .trim()
        .to_lowercase()
        .trim_end_matches('.')
        .to_string()
}

/// Extract and normalize a hostname from a legacy SSH destination
/// (`[user@]host[:port]`, bracketed IPv6 supported).
pub fn ssh_hostname(value: &str) -> String {
    let host_and_port = value.trim().rsplit('@').next().unwrap_or("");
    let host = if host_and_port.starts_with('[') {
        match host_and_port.find(']') {
            Some(closing) if closing > 1 => &host_and_port[1..closing],
            _ => "",
        }
    } else {
        host_and_port.split(':').next().unwrap_or("")
    };
    normalize_hostname(host)
}

// ---------------------------------------------------------------------------
// validation.py — registry-v2 contract
// ---------------------------------------------------------------------------

/// Schema version required of registry documents (Python
/// `_REGISTRY_VERSION`).
pub const REGISTRY_SCHEMA_VERSION: i64 = 2;

/// Raised when a registry does not satisfy the version 2 contract.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct RegistryValidationError(pub String);

fn verr(location: &str, message: &str) -> RegistryValidationError {
    RegistryValidationError(format!("{location}: {message}"))
}

/// Python `repr()` of a sorted string list: `['a', 'b']`.
fn py_list_repr(items: &[&str]) -> String {
    let quoted: Vec<String> = items.iter().map(|i| format!("'{i}'")).collect();
    format!("[{}]", quoted.join(", "))
}

/// `^[a-z0-9](?:[a-z0-9._-]*[a-z0-9])?$` hand-rolled (no regex dependency).
fn is_target_name(value: &str) -> bool {
    let bytes = value.as_bytes();
    let alnum = |b: u8| b.is_ascii_lowercase() || b.is_ascii_digit();
    !bytes.is_empty()
        && alnum(bytes[0])
        && alnum(bytes[bytes.len() - 1])
        && bytes
            .iter()
            .all(|&b| alnum(b) || matches!(b, b'.' | b'_' | b'-'))
}

/// `^[a-z0-9_]+$` hand-rolled.
fn is_action(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
}

fn validate_action_list(value: &Value, location: &str) -> Result<(), RegistryValidationError> {
    let items = value
        .as_array()
        .ok_or_else(|| verr(location, "must be an array"))?;
    let mut seen: HashSet<&str> = HashSet::new();
    for (index, action) in items.iter().enumerate() {
        let item_location = format!("{location}[{index}]");
        let action = match action.as_str() {
            Some(a) if !a.is_empty() && a == a.trim() => a,
            _ => {
                return Err(verr(
                    &item_location,
                    "must be a non-empty string without surrounding whitespace",
                ))
            }
        };
        if !is_action(action) {
            return Err(verr(
                &item_location,
                "must be an exact lowercase action identifier; wildcard grants are forbidden",
            ));
        }
        if !seen.insert(action) {
            return Err(verr(
                &item_location,
                &format!("duplicate action '{action}'"),
            ));
        }
    }
    Ok(())
}

fn require_int(
    value: &Value,
    location: &str,
    minimum: i64,
    maximum: Option<i64>,
) -> Result<i64, RegistryValidationError> {
    // JSON booleans/strings fail as_i64, matching Python's isinstance check.
    let int = value
        .as_i64()
        .ok_or_else(|| verr(location, "must be an integer"))?;
    if int < minimum || maximum.is_some_and(|max| int > max) {
        let upper = maximum.map_or(String::new(), |max| format!(" and <= {max}"));
        return Err(verr(location, &format!("must be >= {minimum}{upper}")));
    }
    Ok(int)
}

fn validate_disk_cleanup(value: &Value, location: &str) -> Result<(), RegistryValidationError> {
    let map = value
        .as_object()
        .ok_or_else(|| verr(location, "must be an object"))?;
    const REQUIRED: [&str; 8] = [
        "check_interval_seconds",
        "cleaners",
        "low_free_gb",
        "max_bytes_per_pass",
        "max_items_per_pass",
        "max_scan_items",
        "mode",
        "target_free_gb",
    ];
    let keys: HashSet<&str> = map.keys().map(String::as_str).collect();
    if keys != REQUIRED.into_iter().collect() {
        return Err(verr(
            location,
            &format!("must contain exactly {}", py_list_repr(&REQUIRED)),
        ));
    }
    let mode_location = format!("{location}.mode");
    if !matches!(map["mode"].as_str(), Some("off" | "report" | "enforce")) {
        return Err(verr(
            &mode_location,
            "must be one of 'off', 'report', or 'enforce'",
        ));
    }
    require_int(
        &map["check_interval_seconds"],
        &format!("{location}.check_interval_seconds"),
        60,
        Some(86400),
    )?;
    let low = require_int(
        &map["low_free_gb"],
        &format!("{location}.low_free_gb"),
        1,
        None,
    )?;
    let target = require_int(
        &map["target_free_gb"],
        &format!("{location}.target_free_gb"),
        1,
        None,
    )?;
    if target <= low {
        return Err(verr(
            &format!("{location}.target_free_gb"),
            "must be greater than low_free_gb",
        ));
    }
    require_int(
        &map["max_bytes_per_pass"],
        &format!("{location}.max_bytes_per_pass"),
        1024_i64.pow(2),
        Some(1024_i64.pow(4)),
    )?;
    let max_items = require_int(
        &map["max_items_per_pass"],
        &format!("{location}.max_items_per_pass"),
        1,
        Some(10000),
    )?;
    let max_scan = require_int(
        &map["max_scan_items"],
        &format!("{location}.max_scan_items"),
        1,
        Some(100000),
    )?;
    if max_scan < max_items {
        return Err(verr(
            &format!("{location}.max_scan_items"),
            "must be >= max_items_per_pass",
        ));
    }
    let cleaners_location = format!("{location}.cleaners");
    let cleaners = map["cleaners"]
        .as_object()
        .ok_or_else(|| verr(&cleaners_location, "must be an object"))?;
    const ALLOWED: [&str; 4] = [
        "build_caches",
        "chromium_clones",
        "huggingface_cache",
        "weles_recordings",
    ];
    let mut unknown: Vec<&str> = cleaners
        .keys()
        .map(String::as_str)
        .filter(|k| !ALLOWED.contains(k))
        .collect();
    unknown.sort_unstable();
    if !unknown.is_empty() {
        return Err(verr(
            &cleaners_location,
            &format!("unknown cleaners {}", py_list_repr(&unknown)),
        ));
    }
    for (name, cleaner) in cleaners {
        let cleaner_location = format!("{cleaners_location}.{name}");
        let cleaner = cleaner
            .as_object()
            .ok_or_else(|| verr(&cleaner_location, "must be an object"))?;
        const CLEANER_KEYS: [&str; 3] = ["allow_missing_upload_proof", "min_age_seconds", "root"];
        let mut unknown_keys: Vec<&str> = cleaner
            .keys()
            .map(String::as_str)
            .filter(|k| !CLEANER_KEYS.contains(k))
            .collect();
        unknown_keys.sort_unstable();
        if !unknown_keys.is_empty() {
            return Err(verr(
                &cleaner_location,
                &format!("unknown keys {}", py_list_repr(&unknown_keys)),
            ));
        }
        let min_age = cleaner
            .get("min_age_seconds")
            .ok_or_else(|| verr(&cleaner_location, "must contain 'min_age_seconds'"))?;
        // Per-cleaner floor on retention. The HF cache is content-addressed
        // and re-downloadable within the hour, so an hour is enough there.
        // A build tree and a weles run both need a day: a `target/` younger
        // than that is the working set of a build someone is still waiting
        // on, and its CACHEDIR.TAG says only that it is reproducible, not
        // that it is idle. A Chromium code-sign clone needs the same day,
        // for the reason that floor exists at all: macOS makes the clone at
        // launch and records nothing about which process owns it, so age is
        // most of what says the launch is over.
        let minimum = if name == "huggingface_cache" {
            3600
        } else {
            86400
        };
        require_int(
            min_age,
            &format!("{cleaner_location}.min_age_seconds"),
            minimum,
            None,
        )?;
        if let Some(proof) = cleaner.get("allow_missing_upload_proof") {
            if !proof.is_boolean() {
                return Err(verr(
                    &format!("{cleaner_location}.allow_missing_upload_proof"),
                    "must be a boolean",
                ));
            }
        }
        if let Some(root) = cleaner.get("root") {
            if root.as_str().is_none_or(|r| r.trim().is_empty()) {
                return Err(verr(
                    &format!("{cleaner_location}.root"),
                    "must be a non-empty string",
                ));
            }
        }
    }
    Ok(())
}

/// (identity, location) pairs declared by one target.
fn target_identities(
    target: &Map<String, Value>,
    location: &str,
) -> Result<Vec<(String, String)>, RegistryValidationError> {
    let mut identities: Vec<(String, String)> = Vec::new();
    let name = target["name"].as_str().unwrap_or("");
    identities.push((normalize_hostname(name), format!("{location}.name")));

    let hostnames_location = format!("{location}.hostnames");
    if let Some(hostnames) = target.get("hostnames") {
        let hostnames = hostnames
            .as_array()
            .ok_or_else(|| verr(&hostnames_location, "must be an array"))?;
        for (index, hostname) in hostnames.iter().enumerate() {
            let item_location = format!("{hostnames_location}[{index}]");
            let hostname = hostname
                .as_str()
                .ok_or_else(|| verr(&item_location, "must be a string"))?;
            let normalized = normalize_hostname(hostname);
            if normalized.is_empty() {
                return Err(verr(&item_location, "must not be empty"));
            }
            if hostname != normalized {
                return Err(verr(
                    &item_location,
                    &format!("must be normalized as '{normalized}'"),
                ));
            }
            if normalized.chars().any(char::is_whitespace)
                || normalized.contains('@')
                || normalized.contains('/')
            {
                return Err(verr(
                    &item_location,
                    "must be a hostname, not a URL or SSH destination",
                ));
            }
            identities.push((normalized, item_location));
        }
    }

    if let Some(ssh) = target.get("ssh") {
        if !ssh.is_null() {
            let ssh = ssh
                .as_str()
                .ok_or_else(|| verr(&format!("{location}.ssh"), "must be a string or null"))?;
            let ssh_identity = ssh_hostname(ssh);
            if ssh_identity.is_empty() {
                return Err(verr(&format!("{location}.ssh"), "must include a host"));
            }
            identities.push((ssh_identity, format!("{location}.ssh")));
        }
    }
    Ok(identities)
}

fn is_product_identifier(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() <= 128
        && bytes.first().is_some_and(u8::is_ascii_alphabetic)
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

/// `owner/name`, both halves non-empty and spelled the way a git forge
/// spells them. The onboarding block's `repository` is the one field in the
/// registry that names a forge repository.
fn is_repository(value: &str) -> bool {
    let valid = |part: &str| {
        !part.is_empty()
            && part
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
    };
    let mut parts = value.split('/');
    matches!(
        (parts.next(), parts.next(), parts.next()),
        (Some(owner), Some(repository), None) if valid(owner) && valid(repository)
    )
}

fn validate_service_onboarding(
    target: &Map<String, Value>,
    location: &str,
) -> Result<(), RegistryValidationError> {
    let Some(services) = target.get("services") else {
        return Ok(());
    };
    let services = services
        .as_array()
        .ok_or_else(|| verr(&format!("{location}.services"), "must be an array"))?;
    for (index, service) in services.iter().enumerate() {
        let service_location = format!("{location}.services[{index}]");
        let service = service
            .as_object()
            .ok_or_else(|| verr(&service_location, "must be an object"))?;
        let Some(onboarding) = service.get("onboarding") else {
            continue;
        };
        let onboarding_location = format!("{service_location}.onboarding");
        let onboarding = onboarding
            .as_object()
            .ok_or_else(|| verr(&onboarding_location, "must be an object"))?;
        const KEYS: [&str; 7] = [
            "display_name",
            "first_success_fact",
            "onboarding_kind",
            "product_id",
            "repository",
            "status",
            "surface_kinds",
        ];
        let keys: HashSet<&str> = onboarding.keys().map(String::as_str).collect();
        if keys != KEYS.into_iter().collect() {
            return Err(verr(
                &onboarding_location,
                &format!("must contain exactly {}", py_list_repr(&KEYS)),
            ));
        }
        for field in ["product_id", "first_success_fact"] {
            if !onboarding[field]
                .as_str()
                .is_some_and(is_product_identifier)
            {
                return Err(verr(
                    &format!("{onboarding_location}.{field}"),
                    "must be a product identifier",
                ));
            }
        }
        if !onboarding["display_name"]
            .as_str()
            .is_some_and(|value| !value.trim().is_empty() && value.len() <= 512)
        {
            return Err(verr(
                &format!("{onboarding_location}.display_name"),
                "must be a non-empty string of at most 512 bytes",
            ));
        }
        if !onboarding["repository"].as_str().is_some_and(is_repository) {
            return Err(verr(
                &format!("{onboarding_location}.repository"),
                "must be an owner/repository identifier",
            ));
        }
        let surfaces = onboarding["surface_kinds"].as_array().ok_or_else(|| {
            verr(
                &format!("{onboarding_location}.surface_kinds"),
                "must be an array",
            )
        })?;
        const SURFACES: [&str; 10] = [
            "web", "ios", "android", "macos", "desktop", "cli", "api", "worker", "operator",
            "python",
        ];
        let mut seen = HashSet::new();
        if surfaces.is_empty()
            || surfaces.iter().any(|surface| {
                !surface
                    .as_str()
                    .is_some_and(|value| SURFACES.contains(&value) && seen.insert(value))
            })
        {
            return Err(verr(
                &format!("{onboarding_location}.surface_kinds"),
                "must contain unique supported surfaces",
            ));
        }
        if !matches!(
            onboarding["onboarding_kind"].as_str(),
            Some("human" | "machine" | "both")
        ) {
            return Err(verr(
                &format!("{onboarding_location}.onboarding_kind"),
                "must be human, machine, or both",
            ));
        }
        if !matches!(
            onboarding["status"].as_str(),
            Some("planned" | "active" | "archived")
        ) {
            return Err(verr(
                &format!("{onboarding_location}.status"),
                "must be planned, active, or archived",
            ));
        }
    }
    Ok(())
}

/// Validate a registry-v2 document without modifying it. Python returns the
/// input dict; here the borrowed input simply remains valid on `Ok(())`.
pub fn validate_registry(data: &Value) -> Result<(), RegistryValidationError> {
    let root = data
        .as_object()
        .ok_or_else(|| verr("registry", "must be an object"))?;
    let version_ok = root
        .get("schema_version")
        .is_some_and(|v| !v.is_boolean() && v.as_i64() == Some(REGISTRY_SCHEMA_VERSION));
    if !version_ok {
        return Err(verr(
            "registry.schema_version",
            &format!("must be {REGISTRY_SCHEMA_VERSION}"),
        ));
    }

    let targets = root
        .get("targets")
        .and_then(Value::as_array)
        .ok_or_else(|| verr("registry.targets", "must be an array"))?;

    let mut names: HashSet<&str> = HashSet::new();
    let mut identities: HashMap<String, String> = HashMap::new();
    let mut target_heuristics: HashMap<&str, &str> = HashMap::new();
    let mut coordinator_heuristics: HashSet<&str> = HashSet::new();
    let valid_kinds =
        crate::capabilities::configurable_ids(crate::capabilities::RuntimeFacet::HostTarget)
            .collect::<Vec<_>>();
    for (index, target) in targets.iter().enumerate() {
        let location = format!("registry.targets[{index}]");
        let target = target
            .as_object()
            .ok_or_else(|| verr(&location, "must be an object"))?;

        let name_location = format!("{location}.name");
        let name = match target.get("name").and_then(Value::as_str) {
            Some(name) if is_target_name(name) => name,
            _ => {
                return Err(verr(
                    &name_location,
                    "must be a lowercase target identifier",
                ))
            }
        };
        if !names.insert(name) {
            return Err(verr(
                &name_location,
                &format!("duplicate target name '{name}'"),
            ));
        }

        let kind = target.get("kind").and_then(Value::as_str).unwrap_or("");
        if !valid_kinds.contains(&kind) {
            return Err(verr(
                &format!("{location}.kind"),
                &format!("must be one of {}", py_list_repr(&valid_kinds)),
            ));
        }
        if let Some(value) = target.get("gpu_power_limit_watts") {
            let watts = value.as_u64().filter(|watts| *watts > 0).ok_or_else(|| {
                verr(
                    &format!("{location}.gpu_power_limit_watts"),
                    "must be a positive integer",
                )
            })?;
            if u32::try_from(watts).is_err() {
                return Err(verr(
                    &format!("{location}.gpu_power_limit_watts"),
                    "must fit in an unsigned 32-bit integer",
                ));
            }
            if !crate::capabilities::ProviderId::Local.matches(kind) {
                return Err(verr(
                    &format!("{location}.gpu_power_limit_watts"),
                    "is allowed only for kind='local'",
                ));
            }
        }
        let platform_location = format!("{location}.release_platform");
        let platform = target
            .get("release_platform")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if !crate::deploy::products::PLATFORMS.contains(&platform) {
            return Err(verr(
                &platform_location,
                &format!(
                    "must be one of {} and must be confirmed by host inventory",
                    py_list_repr(crate::deploy::products::PLATFORMS)
                ),
            ));
        }
        if let Some(role) = target.get("role") {
            if !role.as_str().is_some_and(is_target_name) {
                return Err(verr(
                    &format!("{location}.role"),
                    "must be a lowercase target identifier",
                ));
            }
        }
        if let Some(heuristic) = target.get("host_heuristic") {
            let heuristic_location = format!("{location}.host_heuristic");
            let Some(heuristic) = heuristic.as_str() else {
                return Err(verr(&heuristic_location, "must be a string"));
            };
            if heuristic != "always-on" {
                return Err(verr(
                    &heuristic_location,
                    "must be the supported selector 'always-on'",
                ));
            }
            if !crate::capabilities::ProviderId::Local.matches(kind) {
                return Err(verr(
                    &heuristic_location,
                    "is allowed only for kind='local'",
                ));
            }
            if let Some(previous) = target_heuristics.insert(heuristic, name) {
                return Err(verr(
                    &heuristic_location,
                    &format!("selector '{heuristic}' is already declared by target '{previous}'"),
                ));
            }
        }

        if let Some(services) = target.get("services") {
            let services_location = format!("{location}.services");
            let services = services
                .as_array()
                .ok_or_else(|| verr(&services_location, "must be an array"))?;
            for (service_index, service) in services.iter().enumerate() {
                let service_location = format!("{services_location}[{service_index}]");
                let service = service
                    .as_object()
                    .ok_or_else(|| verr(&service_location, "must be an object"))?;
                if let Some(heuristic) = service.get("host_heuristic") {
                    let heuristic = heuristic.as_str().ok_or_else(|| {
                        verr(
                            &format!("{service_location}.host_heuristic"),
                            "must be a string",
                        )
                    })?;
                    if target.get("host_heuristic").and_then(Value::as_str) != Some(heuristic) {
                        return Err(verr(
                            &format!("{service_location}.host_heuristic"),
                            "must match the containing target's host_heuristic",
                        ));
                    }
                }
            }
        }

        if let Some(weles) = target.get("weles") {
            let weles_location = format!("{location}.weles");
            if !crate::capabilities::ProviderId::Local.matches(kind) {
                return Err(verr(&weles_location, "is allowed only for kind='local'"));
            }
            let weles = weles
                .as_object()
                .ok_or_else(|| verr(&weles_location, "must be an object"))?;
            const WELES_KEYS: [&str; 3] = ["actions", "enabled", "recordings_dir"];
            let mut unknown: Vec<&str> = weles
                .keys()
                .map(String::as_str)
                .filter(|k| !WELES_KEYS.contains(k))
                .collect();
            unknown.sort_unstable();
            if !unknown.is_empty() {
                return Err(verr(
                    &weles_location,
                    &format!("unknown keys {}", py_list_repr(&unknown)),
                ));
            }
            if !weles.contains_key("enabled") || !weles.contains_key("actions") {
                return Err(verr(
                    &weles_location,
                    "must contain 'enabled' and 'actions'",
                ));
            }
            if !weles["enabled"].is_boolean() {
                return Err(verr(
                    &format!("{weles_location}.enabled"),
                    "must be a boolean",
                ));
            }
            validate_action_list(&weles["actions"], &format!("{weles_location}.actions"))?;
            if let Some(recordings_dir) = weles.get("recordings_dir") {
                if !recordings_dir.as_str().is_some_and(|r| r.starts_with('/')) {
                    return Err(verr(
                        &format!("{weles_location}.recordings_dir"),
                        "must be an absolute path string",
                    ));
                }
            }
        }
        validate_service_onboarding(target, &location)?;

        if let Some(cleanup) = target.get("disk_cleanup") {
            if !crate::capabilities::ProviderId::Local.matches(kind) {
                return Err(verr(
                    &format!("{location}.disk_cleanup"),
                    "is allowed only for kind='local'",
                ));
            }
            validate_disk_cleanup(cleanup, &format!("{location}.disk_cleanup"))?;
        }

        for (identity, identity_location) in target_identities(target, &location)? {
            if let Some(previous) = identities.get(&identity) {
                return Err(verr(
                    &identity_location,
                    &format!("host identity '{identity}' is already declared by {previous}"),
                ));
            }
            identities.insert(identity, identity_location);
        }
    }
    if let Some(coordinators) = root.get("coordinators") {
        let coordinators = coordinators
            .as_array()
            .ok_or_else(|| verr("registry.coordinators", "must be an array"))?;
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
    }

    crate::placement::validate_registry_contract(data).map_err(RegistryValidationError)?;
    crate::service_resolution::validate_registry_contract(data).map_err(RegistryValidationError)?;
    crate::release_control::validate_registry_contract(data).map_err(RegistryValidationError)?;

    crate::inference::schema::validate(data).map_err(RegistryValidationError)?;
    Ok(())
}

/// Load and validate a registry-v2 JSON file.
pub fn validate_registry_file(path: &Path) -> Result<Value, RegistryValidationError> {
    let text = std::fs::read_to_string(path)
        .map_err(|exc| RegistryValidationError(format!("{}: {exc}", path.display())))?;
    let data: Value = serde_json::from_str(&text)
        .map_err(|exc| RegistryValidationError(format!("{}: {exc}", path.display())))?;
    validate_registry(&data)?;
    Ok(data)
}

// ---------------------------------------------------------------------------
// capabilities.py — provider-neutral workload admission
// ---------------------------------------------------------------------------

/// What a dispatch target declares it can run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TargetCapabilities {
    pub target_id: String,
    pub operating_system: String,
    pub architecture: String,
    pub cpu_cores: i64,
    pub memory_gb: i64,
    pub disk_gb: i64,
    pub accelerator: String,
    pub execution_modes: BTreeSet<String>,
    pub supports_preemptible: bool,
    pub region_selectable: bool,
    pub supports_system_packages: bool,
}

impl Default for TargetCapabilities {
    fn default() -> Self {
        Self {
            target_id: String::new(),
            operating_system: String::new(),
            architecture: String::new(),
            cpu_cores: 0,
            memory_gb: 0,
            disk_gb: 0,
            accelerator: String::new(),
            execution_modes: BTreeSet::from(["stado-agent".to_string()]),
            supports_preemptible: false,
            region_selectable: false,
            supports_system_packages: false,
        }
    }
}

/// Raised by [`AdmissionDecision::require`] (Python `ValueError`).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct AdmissionRejection(pub String);

/// Outcome of [`admit_job`]: every incompatibility, not just the first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdmissionDecision {
    pub accepted: bool,
    pub reasons: Vec<String>,
}

impl AdmissionDecision {
    pub fn require(&self) -> Result<(), AdmissionRejection> {
        if self.accepted {
            Ok(())
        } else {
            Err(AdmissionRejection(self.reasons.join("; ")))
        }
    }
}

/// Return every incompatibility instead of failing at the first field.
pub fn admit_job(job: &Job, target: &TargetCapabilities) -> AdmissionDecision {
    let mut reasons: Vec<String> = Vec::new();
    let required_os = job.platform_os.to_lowercase();
    let required_arch = job.architecture.to_lowercase();
    let required_cpu = job.cpu_cores;
    let required_memory = job.memory_gb;
    let required_disk = job.disk_gb;
    let executor = if job.executor.is_empty() {
        "stado-agent"
    } else {
        job.executor.as_str()
    };
    let gpu_mem = job.gpu_mem_gb;
    let gpu_type = job.gpu_type.as_str();

    if !required_os.is_empty() && required_os != target.operating_system {
        reasons.push(format!(
            "requires os={required_os}, target is {}",
            target.operating_system
        ));
    }
    if !required_arch.is_empty() && required_arch != target.architecture {
        reasons.push(format!(
            "requires architecture={required_arch}, target is {}",
            target.architecture
        ));
    }
    if required_cpu > target.cpu_cores {
        reasons.push(format!(
            "requires {required_cpu} CPU cores, target has {}",
            target.cpu_cores
        ));
    }
    if required_memory > target.memory_gb {
        reasons.push(format!(
            "requires {required_memory} GB memory, target has {}",
            target.memory_gb
        ));
    }
    if required_disk > target.disk_gb {
        reasons.push(format!(
            "requires {required_disk} GB disk, target has {}",
            target.disk_gb
        ));
    }
    // Faithful to the Python: target.accelerator is NOT consulted here —
    // any GPU requirement rejects against a capability set (the declared
    // accelerator string is informational only).
    if gpu_mem > 0 || !gpu_type.is_empty() {
        reasons.push("target has no accelerator".to_string());
    }
    if !target.execution_modes.contains(executor) {
        reasons.push(format!("executor '{executor}' is unsupported"));
    }
    if job.preemptible && !target.supports_preemptible {
        reasons.push("target does not support preemptible lifecycle".to_string());
    }
    if !job.region.is_empty() && !target.region_selectable {
        reasons.push("target region is not selectable".to_string());
    }
    if !job.apt_packages.is_empty() && !target.supports_system_packages {
        reasons.push("target does not support provider-managed system packages".to_string());
    }
    AdmissionDecision {
        accepted: reasons.is_empty(),
        reasons,
    }
}

static BOX_CAPABILITIES: LazyLock<TargetCapabilities> = LazyLock::new(|| TargetCapabilities {
    target_id: "box-linux-sandbox".to_string(),
    operating_system: "linux".to_string(),
    architecture: "x86_64".to_string(),
    cpu_cores: 4,
    memory_gb: 8,
    disk_gb: 80,
    accelerator: String::new(),
    execution_modes: BTreeSet::from([
        "stado-agent".to_string(),
        "box-command".to_string(),
        "box-prompt".to_string(),
    ]),
    supports_preemptible: false,
    region_selectable: false,
    supports_system_packages: false,
});

/// Capability set of the Linux sandbox box (Python `BOX_CAPABILITIES`).
pub fn box_capabilities() -> &'static TargetCapabilities {
    &BOX_CAPABILITIES
}

// ---------------------------------------------------------------------------
// __init__.py — data models
// ---------------------------------------------------------------------------

fn default_slots() -> i64 {
    1
}

fn default_runtime() -> String {
    "daemon".to_string()
}

fn default_interval_seconds() -> i64 {
    180
}

fn default_state_uri() -> String {
    "stado://system/registry".to_string()
}

/// Tolerate explicit JSON null where Python does `d.get(key) or <default>`.
fn de_null_as_default<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Default + Deserialize<'de>,
{
    Ok(Option::<T>::deserialize(deserializer)?.unwrap_or_default())
}

/// Weles worker policy for a local target.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WelesPolicy {
    pub enabled: bool,
    pub actions: Vec<String>,
    /// Where the Weles worker writes run recordings
    /// (WELES_RECORDINGS_ROOT). Optional; when set, the disk cleaner's
    /// weles_recordings.root should point at <recordings_dir> so policy and
    /// writer never drift apart.
    #[serde(default)]
    pub recordings_dir: Option<String>,
}

/// One identity a host is expected to hold, as opposed to one action it may run.
///
/// The distinction is the whole point. `WelesPolicy.actions` answers "may this host
/// do X" -- permission and capacity. It cannot answer "is this the machine where a
/// two-factor prompt for controlyourai@gmail.com will appear", because that is not a
/// permission at all: it is a property the machine either has or has not, granted by
/// a third party and revocable without telling us.
///
/// Routing such work by an action allowlist buries the discovery of a missing
/// identity at the deepest point of the flow -- a browser trajectory waiting for a
/// code no machine will ever display, until it times out. Declaring the binding here
/// lets the fleet refuse at dispatch and name the host that must be enrolled.
///
/// `verified_at` is deliberately not part of the declaration. A binding is a claim
/// until a host observes it, exactly like a release phase.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IdentityBinding {
    /// Identity family, e.g. "apple-account".
    pub kind: String,
    /// The identity itself, e.g. "controlyourai@gmail.com".
    pub identity: String,
    /// Operating-system user holding it, when the identity is per-user rather than
    /// per-machine. An Apple account signed into one macOS user does not make the
    /// other users on that Mac trusted.
    #[serde(default)]
    pub user: Option<String>,
    /// Observed, never declared: when a host last proved it still holds this.
    #[serde(default)]
    pub verified_at: Option<String>,
}

/// One disk cleaner's policy.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DiskCleanerPolicy {
    pub min_age_seconds: i64,
    /// Explicit opt-in to delete weles run directories WITHOUT durable
    /// upload proof (default false: age is reportable but never authorizes
    /// deletion).
    #[serde(default)]
    pub allow_missing_upload_proof: bool,
    /// Absolute path override for the cleaner's scan root (default: the
    /// cleaner's well-known location, e.g. ~/weles/recordings for weles).
    #[serde(default)]
    pub root: Option<String>,
}

/// Disk-cleanup policy for a local target.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DiskCleanupPolicy {
    pub mode: String,
    pub check_interval_seconds: i64,
    pub low_free_gb: i64,
    pub target_free_gb: i64,
    pub max_bytes_per_pass: i64,
    pub max_items_per_pass: i64,
    pub max_scan_items: i64,
    pub cleaners: BTreeMap<String, DiskCleanerPolicy>,
}

/// One routable box. Unknown registry keys land in
/// [`ComputeTarget::extra`] (Python's `extra` dict), via `#[serde(flatten)]`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ComputeTarget {
    pub name: String,
    /// "local" | "gcp" | "vast"
    pub kind: String,
    /// Verified immutable-release coordinate for this host. Enrollment records
    /// it and every inventory compares it with the remote kernel/architecture.
    #[serde(default)]
    pub release_platform: String,
    #[serde(default)]
    pub gpu_type: Option<String>,
    #[serde(default = "default_slots")]
    pub slots: i64,
    #[serde(default)]
    pub ssh: Option<String>,
    #[serde(default)]
    pub region: Option<String>,
    #[serde(default)]
    pub spot: bool,
    #[serde(default)]
    pub max_concurrent: Option<i64>,
    #[serde(default)]
    pub team_id: Option<i64>,
    /// Stable placement class used by operators and service declarations.
    #[serde(default)]
    pub role: Option<String>,
    /// Declarative selector resolved to exactly one local target.
    #[serde(default)]
    pub host_heuristic: Option<String>,
    #[serde(default)]
    pub notes: String,
    #[serde(default, deserialize_with = "de_null_as_default")]
    pub hostnames: Vec<String>,
    #[serde(default)]
    pub weles: Option<WelesPolicy>,
    /// Identities this host is expected to hold. Empty for the ordinary compute
    /// target that holds none.
    #[serde(default, deserialize_with = "de_null_as_default")]
    pub identities: Vec<IdentityBinding>,
    /// Skarbiec item holding this host's machine account, by item id
    /// (`host-account-<name>`). It is the only pointer from a host name to the
    /// credential that logs into that host, so it is modelled rather than left
    /// in [`ComputeTarget::extra`]: host repair has to follow it from Rust, and
    /// `registry doctor` reports an unmodelled, uncatalogued target key as a
    /// declaration with no reader — correctly, while it sits there.
    ///
    /// Read today by `scripts/read-host-account.py`, which resolves the pointer
    /// and fails when the vault holds no such item or the item names a different
    /// host, and by `scripts/put-host-account.py`, which refuses to write a
    /// credential the registry does not point at.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account_ref: Option<String>,
    #[serde(default)]
    pub disk_cleanup: Option<DiskCleanupPolicy>,
    /// Interactive display session this host renders and streams, when it has
    /// one. Read by `cli::stream` and `deploy::stream`; absent means the host is
    /// headless, which is what every host is until somebody declares otherwise.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_stream: Option<crate::stream::schema::DisplayStream>,
    /// env_overrides and agent_args propagate via the GCS registry to
    /// running agents — the agent compares them every poll and
    /// exits-for-restart when they change, so systemd brings it back up
    /// with the new env / CLI flags.
    #[serde(default, deserialize_with = "de_null_as_default")]
    pub env_overrides: Map<String, Value>,
    #[serde(default, deserialize_with = "de_null_as_default")]
    pub agent_args: Vec<String>,
    /// vram_gb is used by the agent to expand its capacity broadcast to
    /// every GCP gpu_type whose required VRAM ≤ this value
    /// (compatibility-list broadcast). Without it, the agent only
    /// advertises gpu_type as-is.
    #[serde(default)]
    pub vram_gb: Option<i64>,
    /// pinned_only=true: this host's agent claims ONLY jobs explicitly
    /// routed to it (Job.pinned_host or coordinator assigned_to). Keeps
    /// shared workstations from picking up stray queue backlog.
    #[serde(default)]
    pub pinned_only: bool,
    /// Required version of each stado-managed binary under `~/.stado/bin`,
    /// keyed by binary name (`stado`, `skarbiec`) and holding the bare
    /// version number (`0.5.1`), never a prefixed banner like
    /// `stado 0.5.1`.
    ///
    /// This is the registry's DECLARATION of target state, and it is the
    /// half that was missing: `stado host inventory` could always read what
    /// a host actually runs, and had nothing to compare it against, so a
    /// host three releases behind looked exactly like a host at the tip.
    /// Optional on purpose — a target that declares nothing is reported as
    /// `undeclared` rather than as drift, so every registry written before
    /// this field existed stays valid.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub managed_versions: BTreeMap<String, String>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl ComputeTarget {
    pub fn provider(&self) -> Option<crate::capabilities::ProviderId> {
        crate::capabilities::variant(crate::capabilities::RuntimeFacet::HostTarget, &self.kind)
            .and_then(|variant| variant.provider)
    }

    pub fn is_provider(&self, provider: crate::capabilities::ProviderId) -> bool {
        self.provider() == Some(provider)
    }

    /// The version the registry requires of one stado-managed binary on
    /// this host, or `None` when it declares none.
    pub fn declared_version(&self, binary: &str) -> Option<&str> {
        self.managed_versions.get(binary).map(String::as_str)
    }

    /// Desired NVIDIA board power cap for this host. The field remains in
    /// `extra` so older Stado binaries preserve it during registry rewrites;
    /// validation above guarantees the accessor cannot observe zero, a
    /// negative value, or an integer wider than the driver accepts.
    pub fn gpu_power_limit_watts(&self) -> Option<u32> {
        self.extra
            .get("gpu_power_limit_watts")
            .and_then(Value::as_u64)
            .and_then(|watts| u32::try_from(watts).ok())
    }
}

/// Where the scheduling tick runs.
///
/// runtime values:
///   gcp_cloud_function   wisent-compute-tick CF + Cloud Scheduler (default).
///   daemon               long-running `wc coordinator` process (any box).
///   cron                 crontab entry that calls `wc coordinator --once`.
///   aws_lambda           reserved.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Coordinator {
    pub name: String,
    #[serde(default = "default_runtime")]
    pub runtime: String,
    /// ssh user@host for daemon/cron, None = local.
    #[serde(default)]
    pub host: Option<String>,
    /// Resolve this coordinator onto the unique local target carrying the
    /// same declarative placement selector.
    #[serde(default)]
    pub host_heuristic: Option<String>,
    #[serde(default = "default_interval_seconds")]
    pub interval_seconds: i64,
    #[serde(default = "default_state_uri")]
    pub state_uri: String,
    #[serde(default)]
    pub active: bool,
    #[serde(default)]
    pub notes: String,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

// ---------------------------------------------------------------------------
// registry.json — service directory and placement profiles
// ---------------------------------------------------------------------------

/// Which host publishes the service directory, and with which binary.
///
/// One authority per fleet: a directory written from two boxes is two
/// directories, and the loser silently serves endpoints nobody is listening
/// on any more.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DirectoryAuthority {
    /// Registry target name of the publishing host.
    pub target: String,
    /// Absolute path of the `stado` binary that publishes from it.
    pub command: String,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// Where one host answers for a service.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ServiceEndpoint {
    pub url: String,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// What one consumer is entitled to ask a service for. The directory is the
/// only place this is written down, so a consumer absent from the map is not
/// authorized rather than unrestricted.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ServiceConsumer {
    #[serde(default, deserialize_with = "de_null_as_default")]
    pub capabilities: Vec<String>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// Speak HTTP and take any answer as proof that something is serving, 401,
/// 404 and 503 included. Health is a different question from existence, and
/// the outage this machinery came from was an endpoint that answered nothing
/// at all.
pub const VERIFY_KIND_HTTP: &str = "http";
/// Open a TCP connection and close it, for an endpoint that speaks no HTTP.
/// It proves a listener is accepting on the address the declaration hands out
/// — which is the whole of what the directory promises for such a service,
/// and everything an HTTP GET would have lied about.
pub const VERIFY_KIND_TCP: &str = "tcp";
/// Probe from every host the directory hands a dial address to: `service
/// directory publish` writes `endpoints[<this host>]` into that host's
/// forward file, so every one of them has been handed something it will one
/// day call and can be held to it. Standby addresses are not in this set —
/// they live in [`Service::standby`], nothing is meant to answer on them
/// yet, and probing one manufactures an outage out of a declaration.
pub const VERIFY_FROM_ENDPOINT_HOLDERS: &str = "endpoint-holders";
/// Probe only where the service claims to serve. For an endpoint no other
/// host is expected to reach — a socket bound behind a local-only guard —
/// where probing from elsewhere manufactures `unreachable` for a service
/// working exactly as declared.
pub const VERIFY_FROM_ACTIVE_HOST: &str = "active-host";
/// Anything at all came back. The only `expect` this build implements, and
/// the reading `service verify` already had.
pub const VERIFY_EXPECT_ANY_RESPONSE: &str = "any-response";

/// The values this build implements. One list per field, read by both the
/// validator and the prober: two lists is how a descriptor becomes valid at
/// validation time and unimplemented at probe time, which is the exact class
/// of gap — a declaration nothing reads — that this field exists to close.
pub const VERIFY_KINDS: [&str; 2] = [VERIFY_KIND_HTTP, VERIFY_KIND_TCP];
/// Vantages this build implements. See [`VERIFY_KINDS`].
pub const VERIFY_FROMS: [&str; 2] = [VERIFY_FROM_ENDPOINT_HOLDERS, VERIFY_FROM_ACTIVE_HOST];
/// Verdicts this build implements. See [`VERIFY_KINDS`].
pub const VERIFY_EXPECTS: [&str; 1] = [VERIFY_EXPECT_ANY_RESPONSE];

fn default_verify_kind() -> String {
    VERIFY_KIND_HTTP.to_string()
}

fn default_verify_from() -> String {
    VERIFY_FROM_ENDPOINT_HOLDERS.to_string()
}

fn default_verify_expect() -> String {
    VERIFY_EXPECT_ANY_RESPONSE.to_string()
}

/// How one declaration is checked against the world, written down beside the
/// declaration itself.
///
/// `service verify` shipped with a single probe wired into it: HTTP GET, from
/// every host holding an endpoint. That is the right question for every entry
/// the directory holds today and the wrong one for the first entry that is not
/// an HTTP service — and the danger is not that such a checker declines, it is
/// that it answers. A Postgres socket understands nothing an HTTP client says
/// and would be called `unreachable` while serving; a service only its own
/// host may dial would be called `unreachable` from four hosts that were never
/// meant to reach it. Both are verdicts on evidence nobody gathered, and a
/// fleet that files those learns to ignore its own reports — the same ending
/// as the silence they replace.
///
/// So the method travels with the declaration: adding a kind of service to the
/// directory means saying how it is seen, in the object that says where it
/// runs. A value this build does not implement yields `unverified` and never a
/// verdict, and [`validate_verification`] raises it against the author while
/// the registry is validated rather than against an operator reading a sweep.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VerifyDescriptor {
    /// What to speak at the endpoint: [`VERIFY_KIND_HTTP`] or
    /// [`VERIFY_KIND_TCP`].
    #[serde(default = "default_verify_kind")]
    pub kind: String,
    /// Which vantage the probe runs from: [`VERIFY_FROM_ENDPOINT_HOLDERS`] or
    /// [`VERIFY_FROM_ACTIVE_HOST`].
    #[serde(default = "default_verify_from")]
    pub from: String,
    /// What counts as proof: [`VERIFY_EXPECT_ANY_RESPONSE`] today.
    #[serde(default = "default_verify_expect")]
    pub expect: String,
    /// Keys this build does not model, kept verbatim. [`Registry::extra`]
    /// exists for the same reason one level up: on 2026-08-04 the canonical
    /// document lost three top-level blocks to a writer that could not name
    /// them, and a descriptor is no safer — a newer publisher's
    /// `verify.timeout_seconds` must survive a rewrite from this checkout.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// `Map<String, Value>` blocks the `Eq` derive because `serde_json` will not
/// promise reflexivity for floats. It holds here regardless:
/// `serde_json::Number::from_f64` rejects NaN, so no NaN can reach a parsed or
/// constructed descriptor. Spelling it out is what lets
/// `service_resolution::ServiceRoute` keep the `Eq` it already had while
/// carrying one of these.
impl Eq for VerifyDescriptor {}

impl Default for VerifyDescriptor {
    fn default() -> Self {
        Self {
            kind: default_verify_kind(),
            from: default_verify_from(),
            expect: default_verify_expect(),
            extra: Map::new(),
        }
    }
}

/// Every problem in one verification descriptor, located for its author.
///
/// A descriptor is a promise that something goes and looks. A `kind` no build
/// implements is a promise nobody keeps, and the prober can only report that
/// one host at a time as `unverified`, inside a sweep somebody has to be
/// reading — the same shape as the twelve-day silence the sweep was written to
/// end. Raising it where the registry is validated puts the complaint in front
/// of the person typing the word.
///
/// Every problem rather than the first: a descriptor with a wrong `kind` and a
/// wrong `from` must not cost two trips through a document that needs a
/// signing key to rewrite.
///
/// NOT called from [`validate_registry`], which checks the raw registry-v2
/// contract and has never modelled the service directory. Directory entries
/// are validated in `service_resolution::validate_registry_contract`, and that
/// is where this is wired in.
pub fn validate_verification(location: &str, descriptor: &VerifyDescriptor) -> Vec<String> {
    [
        ("kind", descriptor.kind.as_str(), VERIFY_KINDS.as_slice()),
        ("from", descriptor.from.as_str(), VERIFY_FROMS.as_slice()),
        (
            "expect",
            descriptor.expect.as_str(),
            VERIFY_EXPECTS.as_slice(),
        ),
    ]
    .into_iter()
    .filter_map(|(field, value, known)| verify_problem(location, field, value, known))
    .collect()
}

/// One field's complaint, naming the word the author wrote and the words this
/// build answers to. The offending value is quoted back because a descriptor
/// is usually wrong by a character.
fn verify_problem(location: &str, field: &str, value: &str, known: &[&str]) -> Option<String> {
    if known.contains(&value) {
        return None;
    }
    Some(format!(
        "{location}.verify.{field}: unknown value '{value}'; this build implements {}",
        py_list_repr(known)
    ))
}

/// One directory entry: where a service currently runs, and who may call it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Service {
    /// Placement profile that relocates this service, when it belongs to one.
    /// Profile members move as a group, in the profile's declared order.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub placement_profile: Option<String>,
    /// launchd/systemd unit that owns this service when no placement profile
    /// does.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub managed_service: Option<String>,
    /// Registry target currently serving it. Consumers resolve
    /// [`Service::endpoints`] with their OWN name, not with this one: the
    /// active host's entry is simply the address that host uses, which for a
    /// loopback service is also where it serves.
    pub active_host: String,
    /// The address a host USES to reach this service, keyed by the machine
    /// ASKING and never by the machine serving. These services bind loopback
    /// on their own box, so "where is Brama" has a different true answer per
    /// client and the directory states each one instead of leaving every
    /// caller to derive it. `service directory publish` writes
    /// `endpoints[<this host>]` into that host's
    /// `~/.stado/forwards/<service>.local`, which is the file consumers on it
    /// actually read — so an entry here is a promise to that host that the
    /// address works from where it stands.
    ///
    /// One meaning only, now. This comment used to say that a host carrying
    /// an endpoint is not thereby serving, which reads the map as "where each
    /// host would serve", while `publish` handed the same string out as a
    /// number to dial. Both readings survived the type, so on 2026-08-11
    /// `service verify` reported `brama` unreachable on a laptop that merely
    /// stands by for it and the entry had to be silenced by hand. The other
    /// meaning now has [`Service::standby`] and this one has nothing else to
    /// mean.
    #[serde(default)]
    pub endpoints: BTreeMap<String, ServiceEndpoint>,
    /// The address a host would serve on if the service moved there — which
    /// is not a promise that anything answers there now.
    ///
    /// A standby host is by definition not running the service, so silence on
    /// this address is the declared state and not a fault. Nothing may probe
    /// it and call the result a verdict: `service verify` lists these as
    /// `unverified` rows so the address is visible before the move rather
    /// than during it, and never counts one as a failure. Read it through
    /// [`Service::standby_for`], the dial address through
    /// [`Service::address_for`], and neither ever falls back to the other.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub standby: BTreeMap<String, ServiceEndpoint>,
    #[serde(default)]
    pub consumers: BTreeMap<String, ServiceConsumer>,
    /// How this declaration is observed, when its author said. Read it through
    /// [`Service::verification`], never directly: absent means the derived
    /// default, and a reader that treats absent as "do not check" reinstates
    /// the unverifiable declaration.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verify: Option<VerifyDescriptor>,
    /// The deployable half of the declaration: where the bytes come from and
    /// what the unit runs. Absent on entries declared before the contract
    /// existed; older builds keep it verbatim in `extra`, so no writer drops
    /// it silently.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub declaration: Option<crate::declaration::ServiceDeclaration>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl Service {
    /// How to check this declaration: what the author wrote, or the default
    /// derived for them.
    ///
    /// Deriving instead of requiring is deliberate, and the reason is custody
    /// rather than convenience. A required field is a gate, and the key to
    /// this one is out of reach: the canonical registry document cannot be
    /// rewritten without a signing key held elsewhere. "Every service declares
    /// how it is verified" would therefore land as an error against every
    /// entry that already exists, raised by a build that cannot fix a single
    /// one of them — a validator that fails and a verifier that refuses, while
    /// the declarations go on being unchecked. That is a worse position than
    /// the one this replaces, because it looks like progress.
    ///
    /// Derivation makes every declaration verifiable the day this lands and
    /// still leaves writing it down worth doing: the author whose service is
    /// not HTTP-from-every-endpoint-holder says so, and is the only one who
    /// has to.
    ///
    /// The default is exactly the probe `service verify` already ran, so no
    /// entry changes verdict because this field came into existence.
    pub fn verification(&self) -> VerifyDescriptor {
        self.verify.clone().unwrap_or_default()
    }

    /// The address `host` is told to dial, or `None` if the directory hands
    /// it none.
    ///
    /// [`Service::endpoints`] only. A standby address is never a fallback
    /// here: its one declared property is that nothing is listening on it
    /// yet, so returning it would answer "what do I call" with an address
    /// chosen for being dead.
    pub fn address_for(&self, host: &str) -> Option<&ServiceEndpoint> {
        self.endpoints.get(host)
    }

    /// The address `host` would serve on after a move, or `None` if it is not
    /// standing by for this service.
    ///
    /// [`Service::standby`] only, for the same reason in reverse. The pair
    /// exists so that no caller has to decide which map answers its question:
    /// one command reading `endpoints` as "would serve here" while another
    /// read it as "call this" is what cost `brama` a false `unreachable` on
    /// 2026-08-11.
    pub fn standby_for(&self, host: &str) -> Option<&ServiceEndpoint> {
        self.standby.get(host)
    }
}

/// The fleet's service directory: the single answer to "where does X run
/// right now, and may I call it".
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ServiceDirectory {
    pub authority: DirectoryAuthority,
    /// Bumped by the authority on every publication. A consumer that cached
    /// an older generation is holding endpoints that may already point at a
    /// host which has handed the service over — see
    /// [`ServiceDirectoryError::Stale`].
    pub generation: u64,
    #[serde(default)]
    pub services: BTreeMap<String, Service>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// The stable code a machine caller branches on when the service directory it
/// is holding is older than the one the authority published.
///
/// It exists because the alternative was silence: a consumer with a stale
/// directory dials the previous active host, gets a refused connection, and
/// reports "connection refused" — which sends the operator to the network
/// instead of to `stado registry pull`.
pub const SERVICE_DIRECTORY_STALE_CODE: &str = "SERVICE_DIRECTORY_STALE";

/// Why a service could not be resolved from the directory. Every variant is
/// a refusal: a lookup NEVER falls back to a guessed host or a default port,
/// because both produce a call to the wrong process rather than an error.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ServiceDirectoryError {
    #[error(
        "{code}: the cached service directory is generation {cached}, the authority \
         published generation {authority}; refresh it (stado registry pull) and retry — \
         the cached endpoint for '{service}' may name a host that has already handed \
         the service over",
        code = SERVICE_DIRECTORY_STALE_CODE
    )]
    Stale {
        service: String,
        cached: u64,
        authority: u64,
    },
    #[error("service '{0}' is not declared in the service directory")]
    UnknownService(String),
    #[error(
        "service '{service}' declares active host '{host}', which has no endpoint in the \
         service directory"
    )]
    NoEndpoint { service: String, host: String },
}

impl ServiceDirectoryError {
    /// The stable code a machine caller branches on, in the spelling
    /// `machine::MachineError` emits.
    pub fn code(&self) -> &'static str {
        match self {
            Self::Stale { .. } => SERVICE_DIRECTORY_STALE_CODE,
            Self::UnknownService(_) => "SERVICE_NOT_IN_DIRECTORY",
            Self::NoEndpoint { .. } => "SERVICE_ENDPOINT_MISSING",
        }
    }

    /// A stale cache is fixed by re-reading the directory, so the same call
    /// is worth making again. A service the directory does not declare is
    /// not: that one needs an edit.
    pub fn retryable(&self) -> bool {
        matches!(self, Self::Stale { .. })
    }
}

impl ServiceDirectory {
    /// The URL a consumer should call for `service`, fenced against the
    /// generation the authority last published.
    ///
    /// The generation is checked FIRST: a stale directory that happens to
    /// still name a reachable endpoint is the dangerous case, because the
    /// call succeeds against the host that no longer owns the service.
    pub fn endpoint(
        &self,
        service: &str,
        authority_generation: u64,
    ) -> Result<&str, ServiceDirectoryError> {
        self.require_generation(service, authority_generation)?;
        let entry = self
            .services
            .get(service)
            .ok_or_else(|| ServiceDirectoryError::UnknownService(service.to_string()))?;
        entry
            .address_for(&entry.active_host)
            .map(|endpoint| endpoint.url.as_str())
            .ok_or_else(|| ServiceDirectoryError::NoEndpoint {
                service: service.to_string(),
                host: entry.active_host.clone(),
            })
    }

    /// Refuse a directory older than the generation the authority published.
    pub fn require_generation(
        &self,
        service: &str,
        authority_generation: u64,
    ) -> Result<(), ServiceDirectoryError> {
        if self.generation < authority_generation {
            return Err(ServiceDirectoryError::Stale {
                service: service.to_string(),
                cached: self.generation,
                authority: authority_generation,
            });
        }
        Ok(())
    }
}

/// A state file a placement profile must carry with the services it moves.
/// `required` state that is absent aborts the move: half-migrated state is
/// how a vault ends up on the box that is no longer serving it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlacementState {
    pub path: String,
    #[serde(default)]
    pub required: bool,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// The launchd/systemd unit running one service on one host.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlacementUnit {
    pub name: String,
    pub unit: String,
    pub path: String,
    /// "launchd" | "systemd".
    pub kind: String,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// A health check that proves a service came up on the host it moved to.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlacementProbe {
    pub service: String,
    pub url: String,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// What one host needs in order to run a placement profile's services.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlacementHost {
    #[serde(default)]
    pub units: BTreeMap<String, PlacementUnit>,
    #[serde(default)]
    pub probes: Vec<PlacementProbe>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// A group of services that move between hosts together, with the order they
/// stop and start in and the state that travels with them.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlacementProfile {
    pub name: String,
    #[serde(default)]
    pub services: Vec<String>,
    /// Stop order on the host handing over; `start_order` is deliberately
    /// separate rather than the reverse, because a dependency that must stop
    /// last does not always start first.
    #[serde(default)]
    pub stop_order: Vec<String>,
    #[serde(default)]
    pub start_order: Vec<String>,
    #[serde(default)]
    pub state: Vec<PlacementState>,
    #[serde(default)]
    pub hosts: BTreeMap<String, PlacementHost>,
    /// Routing rules the mover rewrites, kept verbatim: this checkout does
    /// not model an entry's shape, and inventing one would delete the parts
    /// it guessed wrong.
    #[serde(default)]
    pub routing: Vec<Value>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

// ---------------------------------------------------------------------------
// registry.json — native build recipes
// ---------------------------------------------------------------------------

/// Top-level registry key holding the fleet's build recipes. Unmodelled by
/// [`Registry`] itself, so the array round-trips through [`Registry::extra`]
/// and a writer of any vintage keeps it.
pub const BUILDS_KEY: &str = "builds";

/// Top-level registry key that, when `true`, halts all build polling
/// fleet-wide (the scheduler's kill switch).
pub const BUILDS_DISABLED_KEY: &str = "builds_disabled";

fn default_build_interval_seconds() -> u64 {
    300
}

/// One release platform's job-routing coordinates: every spelling of its
/// operating system and architecture the fleet writes down, canonical first.
///
/// Two spellings for one machine is not a hypothetical: enrollment reads
/// `uname` (`Darwin`/`arm64`), Rust reads `std::env::consts` (`macos`/
/// `aarch64`), the sandbox box declares `x86_64` and the cloud optimizer
/// accepts `amd64`. A router that knows one of them refuses jobs that name
/// the same host by another word, so every spelling this repository writes
/// belongs in one table rather than at each comparison.
struct PlatformRouting {
    platform: &'static str,
    /// [`Job::platform_os`] spellings; the first is what a build job declares.
    os: &'static [&'static str],
    /// [`Job::architecture`] spellings; the first is what a build job declares.
    arch: &'static [&'static str],
}

/// The platform words of [`crate::deploy::products::PLATFORMS`], each with
/// the job fields that route work to it.
const PLATFORM_ROUTING: [PlatformRouting; 2] = [
    PlatformRouting {
        platform: "darwin-arm64",
        os: &["darwin", "macos"],
        arch: &["arm64", "aarch64"],
    },
    PlatformRouting {
        platform: "linux-amd64",
        os: &["linux"],
        arch: &["amd64", "x86_64"],
    },
];

/// A platform the installer knows but nothing can route work to is a build
/// that submits jobs no host will ever claim. Fail the build of whoever adds
/// the platform word instead.
const _: () = assert!(PLATFORM_ROUTING.len() == crate::deploy::products::PLATFORMS.len());

fn platform_routing(platform: &str) -> Option<&'static PlatformRouting> {
    PLATFORM_ROUTING
        .iter()
        .find(|entry| entry.platform == platform)
}

/// The `(platform_os, architecture)` a job must declare to be routed to
/// `platform`, or `None` when the word is not a release platform.
///
/// The one place a platform word becomes job fields: the build poller and
/// `stado builds run` submit byte-identical routing, and the claiming agent
/// reads the same table back through [`platform_accepts_job`].
pub fn platform_job_os_arch(platform: &str) -> Option<(&'static str, &'static str)> {
    let routing = platform_routing(platform)?;
    Some((routing.os[0], routing.arch[0]))
}

/// Whether a host running `platform` may run a job declaring `platform_os`
/// and `architecture`.
///
/// An empty job field is no constraint — every job submitted before platform
/// routing existed carries two of them, and they stay claimable everywhere.
/// An unknown `platform` accepts nothing constrained: a host the release
/// pipeline does not publish for cannot be the intended target of a job that
/// names a platform.
pub fn platform_accepts_job(platform: &str, platform_os: &str, architecture: &str) -> bool {
    if platform_os.is_empty() && architecture.is_empty() {
        return true;
    }
    let Some(routing) = platform_routing(platform) else {
        return false;
    };
    let names = |spellings: &[&str], value: &str| {
        value.is_empty()
            || spellings
                .iter()
                .any(|spelling| spelling.eq_ignore_ascii_case(value))
    };
    names(routing.os, platform_os) && names(routing.arch, architecture)
}

/// The recorded outcome of one platform's most recent build job.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BuildRun {
    /// "succeeded" | "failed" | "running" | "unclaimable".
    pub status: String,
    /// RFC3339 timestamp of when the run was recorded.
    pub at: String,
    /// Queue job id of the build job. Empty for a run that was refused
    /// before submission (`unclaimable`): there is no job to point at, and
    /// an id that names no job must not be offered as one.
    pub job_id: String,
    /// Store-relative URIs of the uploaded artifacts, empty while running.
    #[serde(default)]
    pub artifact_uris: Vec<String>,
    /// Exact semantic version the built commit carries, when the sha is
    /// tagged; `None` for an untagged commit, which is most of them. A build
    /// never invents one: an untagged commit produces artifacts and no
    /// version, and nothing downstream may be declared from it.
    #[serde(default)]
    pub version: Option<String>,
    /// Whether this run's [`BuildRun::version`] was declared as the fleet's
    /// managed version for this platform (`auto_declare`).
    #[serde(default)]
    pub declared: bool,
    /// Why the run ended the way it did, when one sentence says it better
    /// than the status word alone: the supervision diagnosis on a `failed`
    /// run ("no worker claimed the job within 10m") or the refusal on an
    /// `unclaimable` one. Absent on ordinary outcomes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// One native build recipe: a repository the control plane polls, the
/// platforms it builds for, and the command it runs in a fresh shallow
/// checkout on each of them when the branch head moves.
///
/// Boundary: a build produces artifacts under each job's canonical results
/// URI and, with `auto_declare`, a managed-version declaration for the hosts
/// on the run's platform. It NEVER writes `release_control.products` — a
/// signed release is promoted by `stado release promote`, which verifies the
/// manifest and its signature against a release key. Building a
/// commit is evidence that it compiles; promoting it is a deliberate act on
/// verified artifacts, and collapsing the two would let a poller publish an
/// unsigned release to the fleet.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BuildRecipe {
    /// Unique kebab-case recipe name.
    pub name: String,
    /// HTTPS clone URL of the repository to build.
    pub repo: String,
    /// Branch the poller watches.
    #[serde(rename = "ref")]
    pub branch: String,
    /// Single POSIX sh build command run in the checkout.
    pub command: String,
    /// Paths in the checkout to upload as build artifacts.
    #[serde(default)]
    pub artifacts: Vec<String>,
    /// Release platforms this recipe builds for, one job each. Words from
    /// [`crate::deploy::products::PLATFORMS`]; a recipe with none builds
    /// nothing, and `stado builds add` requires at least one.
    ///
    /// Null-tolerant like every other registry collection: a recipe entry
    /// that does not parse is dropped from [`read_build_recipes`] silently,
    /// so one hand-written `null` would take a recipe out of the poller AND
    /// out of every listing with nothing said.
    #[serde(default, deserialize_with = "de_null_as_default")]
    pub platforms: Vec<String>,
    /// Explicit opt-in: a freshly added recipe never builds until enabled.
    #[serde(default)]
    pub enabled: bool,
    /// Declare a succeeded run's version as the managed version for every
    /// host on that run's platform. Off by default: a repository that builds
    /// on every commit must not move the fleet on every commit.
    #[serde(default)]
    pub auto_declare: bool,
    /// Poll cadence in seconds.
    #[serde(default = "default_build_interval_seconds")]
    pub interval_seconds: u64,
    /// Branch head the poller last enqueued builds for.
    #[serde(default)]
    pub last_seen_ref: Option<String>,
    /// Most recent run per platform, keyed by the platform word. Absent for
    /// a recipe that has never built. See [`BuildRecipe::platforms`] for why
    /// an explicit null reads as empty.
    #[serde(default, deserialize_with = "de_null_as_default")]
    pub runs: BTreeMap<String, BuildRun>,
}

/// The registry's build recipes. An absent `builds` key and entries that do
/// not parse both yield nothing: a recipe this build cannot model must not
/// stop the ones it can.
///
/// Absent keys are absent values, never a refusal — a recipe written before
/// `platforms`, `auto_declare` or `runs` existed parses with them empty (and
/// so builds nothing until an operator names its platforms), and a key this
/// build no longer models is dropped on the next write rather than blocking
/// the read.
pub fn read_build_recipes(registry: &Registry) -> Vec<BuildRecipe> {
    registry
        .extra
        .get(BUILDS_KEY)
        .and_then(Value::as_array)
        .map(|entries| {
            entries
                .iter()
                .filter_map(|entry| serde_json::from_value(entry.clone()).ok())
                .collect()
        })
        .unwrap_or_default()
}

/// Replace the registry's build recipes. The key lives in
/// [`Registry::extra`], so [`Registry::to_document`] carries it into the
/// canonical document like any other unmodelled block.
pub fn write_build_recipes(registry: &mut Registry, recipes: &[BuildRecipe]) {
    registry.extra.insert(
        BUILDS_KEY.to_string(),
        serde_json::to_value(recipes).expect("build recipes serialize infallibly"),
    );
}

// ---------------------------------------------------------------------------
// fleet queue namespace — writer/reader alignment for anything submitted
// ---------------------------------------------------------------------------

/// Top-level registry key recording the queue namespace the fleet's
/// coordinator serves. Unmodelled by [`Registry`] itself, so it round-trips
/// through [`Registry::extra`] like every other key this build does not
/// own.
///
/// It exists because of what happened without it: an operator machine whose
/// `storage.stado.namespace` pointed at a different namespace than the
/// fleet's submitted build jobs that no fleet worker could ever see — the
/// queue namespace a job lands in is a property of the SUBMITTER's config,
/// not of the fleet, so writer and reader silently addressed two queues
/// through one object API. The coordinator records the namespace it serves
/// once per tick ([`record_fleet_queue_namespace`]); submitters compare
/// their ambient namespace against the record
/// ([`fleet_namespace_mismatch`]) and refuse rather than enqueue work into
/// a queue nobody claims.
pub const FLEET_QUEUE_NAMESPACE_KEY: &str = "fleet_queue_namespace";

/// The fleet queue namespace a registry document records, when one has been
/// recorded at all.
pub fn fleet_queue_namespace(document: &Value) -> Option<&str> {
    document
        .get(FLEET_QUEUE_NAMESPACE_KEY)
        .and_then(Value::as_str)
        .filter(|namespace| !namespace.trim().is_empty())
}

/// The operator-facing name of the queue jobs land in on THIS machine: the
/// Stado object namespace on that backend, the queue bucket everywhere
/// else, the backend id when neither is configured. For refusal and
/// diagnosis messages, never for routing.
pub fn queue_name() -> String {
    if crate::capabilities::storage_adapter(crate::config::wc_storage_backend())
        == Some(crate::capabilities::StorageAdapter::StadoObject)
    {
        let namespace = crate::config::wc_stado_storage_namespace();
        if !namespace.trim().is_empty() {
            return namespace.to_string();
        }
    }
    let bucket = crate::config::bucket();
    if !bucket.is_empty() {
        return bucket.to_string();
    }
    crate::config::wc_storage_backend().to_string()
}

/// The one-sentence refusal when this machine's ambient Stado object
/// namespace is not the fleet queue namespace the registry records.
///
/// `None` — no refusal — in three cases: the storage backend has no
/// namespaces (on GCS/Azure/S3/local the bucket locator IS the accessor the
/// coordinator resolves, so both sides of the queue already address one
/// store), the registry records no fleet namespace yet (a fleet whose
/// coordinator predates the record cannot be checked, and refusing would
/// brick submissions against it), or the two agree.
pub fn fleet_namespace_mismatch(document: &Value) -> Option<String> {
    if crate::capabilities::storage_adapter(crate::config::wc_storage_backend())
        != Some(crate::capabilities::StorageAdapter::StadoObject)
    {
        return None;
    }
    let fleet = fleet_queue_namespace(document)?;
    let ambient = crate::config::wc_stado_storage_namespace();
    if fleet == ambient {
        return None;
    }
    Some(format!(
        "build jobs go to the fleet queue namespace {fleet:?} recorded in the registry, \
         but this machine resolves storage.stado.namespace {ambient:?}; set \
         storage.stado.namespace (WC_STADO_STORAGE_NAMESPACE) to {fleet:?} so submitter \
         and fleet claim one queue"
    ))
}

/// Record `namespace` as the fleet queue namespace in the canonical
/// registry document — one fenced raw-document edit, the same pattern the
/// build poller's recipe writes use. Called once per coordinator tick with
/// the namespace the coordinator's own queue store resolves.
///
/// `Ok(true)` only when this call wrote. An up-to-date record, an empty
/// namespace (a non-Stado backend has no queue namespace to record) and a
/// lost compare-and-swap race are all `Ok(false)`: the first two are
/// terminal for the tick, and the race is retried next tick, so none of
/// them is a coordinator log line. Genuine store failures are `Err`.
pub async fn record_fleet_queue_namespace(namespace: &str) -> Result<bool, String> {
    let namespace = namespace.trim();
    if namespace.is_empty() {
        return Ok(false);
    }
    let store = RegistryStore::open()
        .await
        .map_err(|exc| format!("registry store open failed: {exc}"))?;
    let versioned = store
        .read_versioned()
        .await
        .map_err(|exc| format!("registry read failed: {exc}"))?
        .ok_or_else(|| format!("no registry document at {}", store.location()))?;
    let mut document: Value = serde_json::from_str(&versioned.content)
        .map_err(|exc| format!("registry parse failed: {exc}"))?;
    if fleet_queue_namespace(&document) == Some(namespace) {
        return Ok(false);
    }
    document
        .as_object_mut()
        .ok_or_else(|| "registry document is not an object".to_string())?
        .insert(
            FLEET_QUEUE_NAMESPACE_KEY.to_string(),
            Value::String(namespace.to_string()),
        );
    let payload = format!(
        "{}\n",
        serde_json::to_string_pretty(&document)
            .map_err(|exc| format!("registry serialize failed: {exc}"))?
    );
    match store.compare_and_swap(&versioned.version, &payload).await {
        Ok(_) => Ok(true),
        // A concurrent registry writer took the generation. The record is
        // one tick away, never worth a retry loop inside a tick.
        Err(StorageError::StorageConflict(_)) => Ok(false),
        Err(exc) => Err(format!("recording the fleet queue namespace failed: {exc}")),
    }
}

// ---------------------------------------------------------------------------
// __init__.py — loaders (local file, GCS fetch with TTL, source selection)
// ---------------------------------------------------------------------------

/// Canonical GCS location of the registry (Python `GCS_REGISTRY_URI`). Only
/// the "gcs" backend resolves the registry here; every other backend reads
/// [`REGISTRY_BLOB`] from the store `config::wc_storage_backend()` selects.
pub const GCS_REGISTRY_URI: &str = "gs://wisent-compute/registry.json";
/// Store-relative path of the registry document, identical on every
/// backend. `cli::registry` compare-and-swaps this exact path through the
/// configured store, so the read and write sides address one object.
pub const REGISTRY_BLOB: &str = "registry.json";
/// Re-fetch the registry at most this often (Python `_GCS_TTL_SEC`).
pub const GCS_REGISTRY_TTL_SEC: u64 = 30;

/// Path of the registry JSON shipped with the crate (byte-identical copy of
/// `stado/targets/registry.json`).
pub fn bundled_registry_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("data")
        .join("registry.json")
}

/// Registry-load failure (Python raises `ValueError` /
/// `json.JSONDecodeError` at the equivalent sites).
#[derive(Debug, thiserror::Error)]
pub enum RegistryError {
    #[error("invalid registry JSON: {0}")]
    Json(String),
    #[error("failed to read registry file {0}: {1}")]
    Io(PathBuf, std::io::Error),
    #[error("invalid registry entry: {0}")]
    InvalidEntry(String),
    #[error("hostname '{identity}' matches multiple registry targets: {names}")]
    AmbiguousIdentity { identity: String, names: String },
}

/// A parsed registry document: targets, coordinator entries, the service
/// directory, the placement profiles — and, verbatim, every top-level key
/// this build does not model.
///
/// [`Registry::extra`] is load-bearing, not cosmetic. A registry write
/// replaces the WHOLE document, so a writer built from a checkout that does
/// not model a key deletes it for everyone: on 2026-08-04 the canonical
/// document lost `channels`, `enrollment` and `fleets` exactly that way,
/// between one read and the next. Round-tripping the unmodelled keys
/// (`schema_version` and `inference` today) makes serializing a `Registry`
/// back a lossless copy of what was read, whatever the writer's vintage.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Registry {
    pub targets: Vec<ComputeTarget>,
    pub coordinators: Vec<Coordinator>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub service_directory: Option<ServiceDirectory>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub placement_profiles: Vec<PlacementProfile>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
    /// How old the document a reader is holding is, in seconds, or `None`
    /// when it did not come from the on-disk last-known-good cache — either
    /// the authority answered, or [`load_registry_auto`] fell all the way
    /// through to the bundled snapshot and said so in its own sentence.
    ///
    /// Skipped by the serializer deliberately. [`Registry::to_document`] is
    /// what a writer pushes back, and a reader-side observation is not part
    /// of the registry: serialized once, it would come back inside
    /// [`Registry::extra`] on the next read and from there into the
    /// canonical document, which is how `channels` and `fleets` were lost.
    #[serde(skip)]
    pub staleness_seconds: Option<i64>,
}

/// Top-level registry keys this build models; everything else round-trips
/// through [`Registry::extra`].
const MODELLED_TOP_LEVEL_KEYS: [&str; 4] = [
    "targets",
    "coordinators",
    "service_directory",
    "placement_profiles",
];

/// Entries with a truthy `name` survive; the rest are skipped (Python
/// `if isinstance(d, dict) and d.get("name")`).
fn name_is_truthy(value: &Value) -> bool {
    value
        .as_object()
        .and_then(|map| map.get("name"))
        .and_then(Value::as_str)
        .is_some_and(|name| !name.is_empty())
}

fn parse_targets(data: &Value) -> Result<Vec<ComputeTarget>, RegistryError> {
    // Python: raw = data.get("targets") if isinstance(data, dict) else data
    let raw = match data {
        Value::Object(map) => map.get("targets").unwrap_or(&Value::Null),
        other => other,
    };
    let mut targets = Vec::new();
    if let Value::Array(items) = raw {
        for item in items {
            if !name_is_truthy(item) {
                continue;
            }
            targets.push(
                serde_json::from_value(item.clone())
                    .map_err(|exc| RegistryError::InvalidEntry(exc.to_string()))?,
            );
        }
    }
    Ok(targets)
}

fn parse_coordinators(data: &Value) -> Result<Vec<Coordinator>, RegistryError> {
    let mut coordinators = Vec::new();
    if let Value::Object(map) = data {
        if let Some(Value::Array(items)) = map.get("coordinators") {
            for item in items {
                if !name_is_truthy(item) {
                    continue;
                }
                coordinators.push(
                    serde_json::from_value(item.clone())
                        .map_err(|exc| RegistryError::InvalidEntry(exc.to_string()))?,
                );
            }
        }
    }
    Ok(coordinators)
}

/// The service directory, or `None` when the document carries none. A block
/// that IS there and does not parse is an error rather than a `None`: a
/// silently empty directory reads as "no service runs anywhere", which is
/// indistinguishable from a fleet that is down.
fn parse_service_directory(data: &Value) -> Result<Option<ServiceDirectory>, RegistryError> {
    match data
        .as_object()
        .and_then(|map| map.get("service_directory"))
    {
        None | Some(Value::Null) => Ok(None),
        Some(raw) => serde_json::from_value(raw.clone())
            .map(Some)
            .map_err(|exc| RegistryError::InvalidEntry(format!("service_directory: {exc}"))),
    }
}

fn parse_placement_profiles(data: &Value) -> Result<Vec<PlacementProfile>, RegistryError> {
    match data
        .as_object()
        .and_then(|map| map.get("placement_profiles"))
    {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(raw) => serde_json::from_value(raw.clone())
            .map_err(|exc| RegistryError::InvalidEntry(format!("placement_profiles: {exc}"))),
    }
}

/// Every top-level key this build does not model, kept verbatim so a
/// read-modify-write cycle cannot drop it.
fn parse_extra(data: &Value) -> Map<String, Value> {
    let Value::Object(map) = data else {
        return Map::new();
    };
    map.iter()
        .filter(|(key, _)| !MODELLED_TOP_LEVEL_KEYS.contains(&key.as_str()))
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect()
}

/// Parse a registry document from a JSON string.
pub fn load_registry_from_str(text: &str) -> Result<Registry, RegistryError> {
    let data: Value =
        serde_json::from_str(text).map_err(|exc| RegistryError::Json(exc.to_string()))?;
    Ok(Registry {
        targets: parse_targets(&data)?,
        coordinators: parse_coordinators(&data)?,
        service_directory: parse_service_directory(&data)?,
        placement_profiles: parse_placement_profiles(&data)?,
        extra: parse_extra(&data),
        staleness_seconds: None,
    })
}

/// Load a registry from a local JSON file. A missing file yields an empty
/// registry (Python `load_targets` behavior); malformed JSON is an error.
pub fn load_registry_file(path: &Path) -> Result<Registry, RegistryError> {
    if !path.is_file() {
        return Ok(Registry::default());
    }
    let text =
        std::fs::read_to_string(path).map_err(|exc| RegistryError::Io(path.to_path_buf(), exc))?;
    load_registry_from_str(&text)
}

/// Load the registry embedded in every standalone release binary. Keeping the
/// compile-time path only as an operator-facing location helper avoids making
/// installed binaries depend on the build machine's `/app/data` directory.
pub fn load_bundled_registry() -> Result<Registry, RegistryError> {
    load_registry_from_str(include_str!("../data/registry.json"))
}

// ---------------------------------------------------------------------------
// __init__.py — `_load_from_gcs` + source-aware loaders
// ---------------------------------------------------------------------------

/// Short-TTL in-process cache of the fetched registry (Python `_GCS_CACHE`).
static REGISTRY_CACHE: LazyLock<Mutex<Option<(Instant, Registry)>>> =
    LazyLock::new(|| Mutex::new(None));

/// Store-relative `registry.json` download: `Ok(Some(text))` = fetched with
/// the store's generation token, `Ok(None)` = blob absent (Python
/// `blob.generation is None`), `Err(msg)` = the store could not be reached
/// at all.
///
/// The generation travels with the text because the last-known-good cache
/// records WHICH document it is holding ([`RegistryCacheMeta::generation`]):
/// a cache that can only say "some registry, once" cannot be compared with
/// the authority when the authority comes back.
pub type RegistryDownloader =
    Arc<dyn Fn() -> BoxFuture<'static, Result<Option<VersionedText>, String>> + Send + Sync>;

/// Test seam replacing the production download (loopback mocks).
static REGISTRY_DOWNLOADER: LazyLock<Mutex<Option<RegistryDownloader>>> =
    LazyLock::new(|| Mutex::new(None));

/// Install a downloader in place of the production fetch (tests only —
/// `#[doc(hidden)]`, not part of the crate's operational surface). Pair with
/// [`clear_registry_cache`] so a cached document never leaks across
/// tests, and serialize via `testutil::GLOBAL_STATE_LOCK`.
#[doc(hidden)]
pub fn set_registry_downloader_for_testing(downloader: Option<RegistryDownloader>) {
    *REGISTRY_DOWNLOADER
        .lock()
        .expect("registry downloader lock") = downloader;
}

/// Drop the cached registry so the next [`fetch_registry_remote`] call
/// re-downloads. Dashboard policy writes call this immediately after a
/// successful CAS; tests also use it to isolate downloader seams.
pub fn clear_registry_cache() {
    *REGISTRY_CACHE.lock().expect("registry cache lock") = None;
}

/// Read/write handle on the canonical registry document, backend-aware.
///
/// NO Python original: Python pins every registry call site to GCS, which
/// is exactly the failure this type removes. On the "gcs" backend the
/// object is the bucket/blob of [`GCS_REGISTRY_URI`], reached through the
/// crate's GCS JSON API (never gsutil) — byte-identical to the pinned code
/// it replaces. On every other backend it is [`REGISTRY_BLOB`] in the
/// store `config::wc_storage_backend()` selects, which is the same object
/// `dashboard/policy.rs` compare-and-swaps the registry through.
///
/// Readers that only want the parsed document use
/// [`fetch_registry_remote`]. This type is for the WRITE side and for
/// readers that need generation fencing: `cli/registry.rs::push`,
/// `cli/host.rs::weles_recordings_dir` and
/// `providers::local::disk_cleanup::fetch_canonical_registry` all fenced
/// against a hardcoded GCS bucket before, so on an Azure-only deployment
/// they could not repair the very registry the coordinator's survival
/// check reads.
pub struct RegistryStore {
    backend: Arc<dyn BlobBackend>,
    blob: String,
    location: String,
}

impl RegistryStore {
    /// Bind to the store that holds the canonical registry.
    pub async fn open() -> Result<Self, StorageError> {
        let gcs_backend = crate::capabilities::storage_adapter(crate::config::wc_storage_backend())
            == Some(crate::capabilities::StorageAdapter::Gcs);
        if gcs_backend {
            let uri = GCS_REGISTRY_URI
                .strip_prefix("gs://")
                .unwrap_or(GCS_REGISTRY_URI);
            let (bucket, blob) = uri.split_once('/').unwrap_or((uri, REGISTRY_BLOB));
            let backend = crate::queue::GcsBackend::new(bucket).await?;
            return Ok(Self {
                backend: Arc::new(backend),
                blob: blob.to_string(),
                location: GCS_REGISTRY_URI.to_string(),
            });
        }
        let store = JobStorage::new().await?;
        Ok(Self {
            backend: Arc::clone(store.backend()),
            blob: REGISTRY_BLOB.to_string(),
            location: registry_location(),
        })
    }

    /// Operator-facing location of the object this handle addresses, in
    /// the spelling [`RegistryFetchError`] reports.
    pub fn location(&self) -> &str {
        &self.location
    }

    /// Registry text, or `None` when the object does not exist.
    pub async fn read_text(&self) -> Result<Option<String>, StorageError> {
        self.backend.download_text(&self.blob).await
    }

    /// Registry text plus the generation/ETag a compare-and-swap needs.
    pub async fn read_versioned(&self) -> Result<Option<VersionedText>, StorageError> {
        self.backend.download_text_versioned(&self.blob).await
    }

    /// Create the registry object; `false` when one already exists.
    pub async fn create_if_absent(&self, content: &str) -> Result<bool, StorageError> {
        self.backend
            .upload_text_if_absent(&self.blob, content)
            .await
    }

    /// Replace the registry iff its generation still matches; returns the
    /// new generation.
    pub async fn compare_and_swap(
        &self,
        expected_version: &str,
        content: &str,
    ) -> Result<String, StorageError> {
        self.backend
            .compare_and_swap_text(&self.blob, expected_version, content)
            .await
    }

    /// Create one object beside the registry document, never replacing one.
    ///
    /// "Beside" literally: the key is derived from the registry blob's own
    /// prefix on whichever store [`RegistryStore::open`] resolved, so the
    /// record of a registry mutation cannot end up in a different bucket from
    /// the document it is about — which on a GCS deployment, where the
    /// registry has its own bucket and [`crate::queue::JobStorage`] does not,
    /// is exactly what writing it through the queue store would do.
    ///
    /// Create-only, because an audit record that a later write can replace is
    /// not an audit record. Returns the full key and whether it was created.
    pub async fn write_beside(
        &self,
        relative: &str,
        content: &str,
    ) -> Result<(String, bool), StorageError> {
        let key = match self.blob.rsplit_once('/') {
            Some((prefix, _)) => format!("{prefix}/{relative}"),
            None => relative.to_string(),
        };
        let created = self.backend.upload_text_if_absent(&key, content).await?;
        Ok((key, created))
    }
}

/// Production download of the registry document through the store
/// `WC_STORAGE_BACKEND` selects.
///
/// On "gcs" this stays pinned to [`GCS_REGISTRY_URI`]'s own bucket via the
/// crate's [`crate::queue::GcsBackend`] (the GCS JSON API — never gsutil).
/// Every other backend reads [`REGISTRY_BLOB`] from
/// [`crate::queue::JobStorage`]: the same store the rest of the tick uses,
/// and the same one `dashboard/policy.rs` compare-and-swaps the registry
/// through. Python hardcodes GCS, so on an Azure-only deployment its
/// readers sit on a dead object while the dashboard edits the live one.
///
/// Python `_load_from_gcs` uses the GCS Python SDK for the same reason:
/// earlier this shelled out to `gsutil cat`, and on systems with a broken
/// gsutil install (cryptography/pyOpenSSL version mismatch breaking
/// `module 'OpenSSL.crypto' has no attribute 'sign'`) gsutil exited
/// non-zero and the agent crashed with 'hostname X not in registry' even
/// though the registry WAS in GCS — confirmed live on 2026-05-08, when the
/// workstation's gsutil broke after a pip upgrade and knocked the agent
/// offline. The GCS SDK was already a hard dependency; using it directly
/// removes the gsutil binary as a single point of failure.
async fn download_registry_blob() -> Result<Option<VersionedText>, String> {
    // One seam for both directions: [`RegistryStore`] resolves the same
    // object `cli/registry.rs::push` writes and `dashboard/policy.rs`
    // compare-and-swaps, so a reader can never sit on a dead object while
    // the writer edits a live one.
    let store = RegistryStore::open().await.map_err(|exc| exc.to_string())?;
    store.read_versioned().await.map_err(|exc| exc.to_string())
}

async fn download_registry() -> Result<Option<VersionedText>, String> {
    let downloader = REGISTRY_DOWNLOADER
        .lock()
        .expect("registry downloader lock")
        .clone();
    match downloader {
        Some(downloader) => downloader().await,
        None => download_registry_blob().await,
    }
}

/// Why the canonical registry could not be READ — as distinct from a
/// registry that WAS read and simply does not list a given entry.
///
/// The split is load-bearing. The coordinator's rogue-daemon kill switch
/// (`coordinator::run`) exits the process when a registry it successfully
/// read omits its entry, and must keep running when it could not read one
/// at all. Collapsing both into an empty registry is what took the fleet
/// down when the GCP billing account was closed: every GCS call started
/// answering `accountDisabled`, and the kill switch fired fleet-wide
/// against a registry nobody had touched.
#[derive(Debug, thiserror::Error)]
pub enum RegistryFetchError {
    /// The store refused or failed the read (auth, network, disabled
    /// billing account, ...). Says NOTHING about the registry's contents.
    #[error("registry store unreachable ({location}): {detail}")]
    Unreachable {
        /// Where the read was attempted, per `registry_location`.
        location: String,
        /// The underlying store error.
        detail: String,
    },
    /// The store answered, but holds no registry document. Just as
    /// non-authoritative about any single entry: a container nobody has
    /// seeded yet looks exactly like this, and the documented kill switch
    /// is "operator removed the ENTRY", never "operator deleted the whole
    /// registry".
    #[error("no registry document at {location}")]
    Absent {
        /// Where the read was attempted, per `registry_location`.
        location: String,
    },
    /// A document came back that is not a valid registry, so its contents
    /// cannot be trusted to revoke anything.
    #[error("invalid registry document at {location}: {source}")]
    Invalid {
        /// Where the document was read from, per `registry_location`.
        location: String,
        /// The parse failure.
        source: RegistryError,
    },
}

/// Operator-facing location of the registry document: the canonical `gs://`
/// URI on the "gcs" backend, else `<backend>:<blob>` for whichever store
/// `WC_STORAGE_BACKEND` selects. Public so the CLI reports the object it
/// actually wrote instead of a hardcoded bucket
/// (`cli/registry.rs::push`).
pub fn registry_location() -> String {
    let backend = crate::config::wc_storage_backend();
    if crate::capabilities::storage_adapter(backend)
        == Some(crate::capabilities::StorageAdapter::Gcs)
    {
        GCS_REGISTRY_URI.to_string()
    } else {
        format!("{backend}:{REGISTRY_BLOB}")
    }
}

// ---------------------------------------------------------------------------
// last-known-good cache — what a reader answers with when the store is down
// ---------------------------------------------------------------------------

/// Reader-side copy of the last registry document the authority served.
pub const REGISTRY_LAST_GOOD_FILE: &str = "registry-last-good.json";
/// Sidecar dating [`REGISTRY_LAST_GOOD_FILE`] and naming which document it
/// is. Kept beside the copy rather than inside it so the copy stays
/// byte-identical to what the authority served.
pub const REGISTRY_LAST_GOOD_META_FILE: &str = "registry-last-good.meta.json";

/// When the cached registry was read, and which document it is.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegistryCacheMeta {
    /// RFC3339 instant the authority answered with this document.
    pub read_at: String,
    /// The store's generation/ETag for that document.
    pub generation: String,
}

/// `~/.stado/cache`, or `None` when `HOME` is unset — a daemon started with
/// no environment still reads the registry, it just gets no cache rather
/// than a cache in whatever directory it happened to start in.
fn registry_cache_dir() -> Option<PathBuf> {
    Some(
        Path::new(&std::env::var_os("HOME")?)
            .join(".stado")
            .join("cache"),
    )
}

/// Path of the last-known-good registry document.
pub fn registry_last_good_path() -> Option<PathBuf> {
    Some(registry_cache_dir()?.join(REGISTRY_LAST_GOOD_FILE))
}

/// Path of the sidecar that dates the last-known-good document.
pub fn registry_last_good_meta_path() -> Option<PathBuf> {
    Some(registry_cache_dir()?.join(REGISTRY_LAST_GOOD_META_FILE))
}

/// Write `content` through a per-process temp file and one rename, so a
/// crash leaves the previous file rather than half of the next one.
fn write_atomic(path: &Path, content: &str) -> std::io::Result<()> {
    let mut temp = path.to_path_buf();
    // Per-process, because two stado invocations refreshing the cache in the
    // same second must not interleave their bytes in one temp file. The
    // rename can still be lost by the other writer — both wrote the same
    // document, so losing it costs nothing.
    temp.set_extension(format!("{}.tmp", std::process::id()));
    std::fs::write(&temp, content)?;
    match std::fs::rename(&temp, path) {
        Ok(()) => Ok(()),
        Err(error) => {
            let _ = std::fs::remove_file(&temp);
            Err(error)
        }
    }
}

/// Record a document the authority served AND this build validated.
///
/// The gate is the registry-v2 contract ([`validate_registry`]), not the
/// loader's tolerance. [`load_registry_from_str`] SKIPS what it cannot model:
/// a `targets` value that is the string `"not-a-list"` leaves it holding zero
/// targets and no complaint, so gating the copy on the loader alone recorded
/// that string as the fleet's last known good registry and then served it —
/// an empty fleet — for as long as the store stayed down. Confirmed by hand
/// on 2026-08-19 against this exact code before the gate moved here.
///
/// A document that fails the contract is reported and not recorded: the copy
/// already on disk is worth more than the newest thing the store happened to
/// hold, and a cache nobody can trust is a cache nobody may use.
///
/// Document first, sidecar second. A crash between the two leaves a new
/// document dated by the older sidecar, which OVERSTATES the age; the other
/// order understates it, and a registry that reads fresher than it is, is
/// the exact lie this cache exists to prevent.
pub(crate) fn store_last_good(text: &str, generation: &str) {
    let (Some(document), Some(meta)) = (registry_last_good_path(), registry_last_good_meta_path())
    else {
        return;
    };
    let report = |error: &dyn std::fmt::Display| {
        eprintln!(
            "[registry-cache] not recording the last-known-good registry in {}: {error}",
            document.display()
        );
    };
    match serde_json::from_str::<Value>(text) {
        Ok(data) => {
            if let Err(error) = validate_registry(&data) {
                report(&error);
                return;
            }
        }
        Err(error) => {
            report(&error);
            return;
        }
    }
    if let Some(directory) = document.parent() {
        if let Err(error) = std::fs::create_dir_all(directory) {
            report(&error);
            return;
        }
    }
    let sidecar = serde_json::to_string(&RegistryCacheMeta {
        read_at: Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true),
        generation: generation.to_string(),
    })
    .expect("registry cache metadata serialization is infallible");
    if let Err(error) = write_atomic(&document, text) {
        report(&error);
        return;
    }
    if let Err(error) = write_atomic(&meta, &sidecar) {
        report(&error);
    }
}

/// The cached document with the age of what it is, or `None` when there is
/// no usable pair on disk.
///
/// Both files or neither: the age is part of the answer, and a document
/// nobody can date is indistinguishable from a document from last year. The
/// contract was checked on the way in ([`store_last_good`]); on the way out
/// the loader is the gate, so a copy truncated by a full disk is skipped
/// rather than served as an empty fleet.
fn load_last_good() -> Option<(Registry, RegistryCacheMeta, i64)> {
    let meta: RegistryCacheMeta =
        serde_json::from_str(&std::fs::read_to_string(registry_last_good_meta_path()?).ok()?)
            .ok()?;
    let read_at = DateTime::parse_from_rfc3339(&meta.read_at)
        .ok()?
        .with_timezone(&Utc);
    let text = std::fs::read_to_string(registry_last_good_path()?).ok()?;
    let mut registry = load_registry_from_str(&text).ok()?;
    let age = (Utc::now() - read_at).num_seconds().max(0);
    registry.staleness_seconds = Some(age);
    Some((registry, meta, age))
}

/// Whether this process has already told the operator it is reading a copy.
static REGISTRY_NOTICE_REPORTED: AtomicBool = AtomicBool::new(false);

/// Print a fallback notice to stderr, once per process.
///
/// Once, because a fleet sweep resolves twenty hosts through one dead
/// authority: twenty copies of the same sentence bury the twenty answers the
/// operator asked for, and the sentence is about the process, not the host.
pub fn report_registry_notice(notice: &str) {
    if !REGISTRY_NOTICE_REPORTED.swap(true, Ordering::Relaxed) {
        eprintln!("{notice}");
    }
}

/// What a degraded registry read is answering from: the authority's own
/// refusal and the identity of the cached copy being served. Carried
/// structured so a caller can word its own one-line notice
/// (`fetch_registry_or_last_good` keeps the historical sentence in
/// [`RegistryCopyNotice::notice`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegistryCopyNotice {
    /// The historical one-sentence notice (age, copy path, read_at,
    /// generation, authority error).
    pub notice: String,
    /// The authority's own error text.
    pub cause: String,
    /// When the authority served the copy being read (RFC3339).
    pub read_at: String,
    /// Age of the copy in seconds.
    pub age_seconds: i64,
}

/// The registry for a reader that must keep answering when the authority
/// does not: the canonical document first, then the last-known-good copy.
///
/// The second element is the one sentence the caller MUST put in front of
/// the operator when the answer came from the copy. It names the age of what
/// is being read and keeps the authority's own error text, because "this
/// registry is 412 s old" and "the store is unreachable" send an operator to
/// two different places — and the behavior this replaces sent them to
/// neither: every host command died with one line about the store while the
/// question was which host went silent.
///
/// A `Some` sentence and `Registry::staleness_seconds` move together. `Err`
/// means the authority failed and there is no usable copy; the bundled
/// snapshot is below this, in [`load_registry_auto`].
pub async fn fetch_registry_or_last_good() -> Result<(Registry, Option<String>), RegistryFetchError>
{
    let (registry, copy) = fetch_registry_or_last_good_detail().await?;
    Ok((registry, copy.map(|copy| copy.notice)))
}

/// [`fetch_registry_or_last_good`] with the fallback carried structured:
/// the authority's refusal and the copy's identity as separate fields, for
/// callers whose operator contract names its own sentence.
pub async fn fetch_registry_or_last_good_detail(
) -> Result<(Registry, Option<RegistryCopyNotice>), RegistryFetchError> {
    let authority = match fetch_registry_remote().await {
        Ok(registry) => return Ok((registry, None)),
        Err(error) => error,
    };
    match load_last_good() {
        Some((registry, meta, age)) => {
            let notice = format!(
                "reading the last-known-good registry copy from {age}s ago ({}, read_at {}, generation {}) because the authority did not answer: {authority}",
                registry_last_good_path().unwrap_or_default().display(),
                meta.read_at,
                meta.generation,
            );
            Ok((
                registry,
                Some(RegistryCopyNotice {
                    notice,
                    cause: authority.to_string(),
                    read_at: meta.read_at,
                    age_seconds: age,
                }),
            ))
        }
        None => Err(authority),
    }
}

/// Fetch the canonical registry from the configured store (Python
/// `_load_from_gcs`, `source="gcs"`): the authority for fleet-survival
/// decisions — the coordinator's rogue-daemon kill switch and host-health
/// target resolution. Cached for [`GCS_REGISTRY_TTL_SEC`] seconds; only
/// successful fetches are cached, so a failure is retried on the next call
/// (Python parity).
///
/// Returns [`RegistryFetchError`] rather than an empty registry: a caller
/// MUST NOT read "the store is unreachable" as "the entry is gone". There
/// is still no local escape hatch here — the bundled file is reachable only
/// through [`load_registry_auto`].
pub async fn fetch_registry_remote() -> Result<Registry, RegistryFetchError> {
    if let Some((ts, registry)) = &*REGISTRY_CACHE.lock().expect("registry cache lock") {
        if ts.elapsed() < Duration::from_secs(GCS_REGISTRY_TTL_SEC) {
            return Ok(registry.clone());
        }
    }
    let fetched = fetch_registry_remote_uncached().await;
    if let Ok(registry) = &fetched {
        *REGISTRY_CACHE.lock().expect("registry cache lock") =
            Some((Instant::now(), registry.clone()));
    }
    fetched
}

async fn fetch_registry_remote_uncached() -> Result<Registry, RegistryFetchError> {
    let location = registry_location();
    match download_registry().await {
        Ok(Some(document)) => match load_registry_from_str(&document.content) {
            Ok(registry) => {
                store_last_good(&document.content, &document.version);
                Ok(registry)
            }
            // The `[_load_from_gcs]` prefix is Python's function name, kept
            // verbatim so existing operator log greps still match.
            Err(source) => {
                eprintln!("[_load_from_gcs] failed: {source}");
                Err(RegistryFetchError::Invalid { location, source })
            }
        },
        // Ok(None) = blob absent (Python `blob.generation is None`).
        Ok(None) => Err(RegistryFetchError::Absent { location }),
        Err(detail) => {
            eprintln!("[_load_from_gcs] failed: {detail}");
            Err(RegistryFetchError::Unreachable { location, detail })
        }
    }
}

/// The registry document, authority first, then the last-known-good copy,
/// then the file bundled with this binary (Python `load_targets` /
/// `load_coordinators` with `source="auto"`). Every [`RegistryFetchError`]
/// falls through.
///
/// Announces which of the three it read whenever that is not the authority:
/// the bundled snapshot is a build artifact that can be months old, and
/// answering out of it in silence is how a decommissioned host stayed
/// "declared" for a fortnight.
pub async fn load_registry_auto() -> Result<Registry, RegistryError> {
    match fetch_registry_or_last_good().await {
        Ok((registry, notice)) => {
            if let Some(notice) = notice {
                report_registry_notice(&notice);
            }
            Ok(registry)
        }
        Err(authority) => {
            report_registry_notice(&format!(
                "reading the registry snapshot bundled with this binary because the authority did not answer and there is no last-known-good copy at {}: {authority}",
                registry_last_good_path().unwrap_or_default().display(),
            ));
            load_bundled_registry()
        }
    }
}

impl Registry {
    /// Return the named target, or None if not in the registry.
    pub fn lookup(&self, name: &str) -> Option<&ComputeTarget> {
        self.targets.iter().find(|t| t.name == name)
    }

    /// Subset of targets with kind='local'. Used by wc bootstrap.
    pub fn local_targets(&self) -> Vec<&ComputeTarget> {
        self.targets
            .iter()
            .filter(|target| target.is_provider(crate::capabilities::ProviderId::Local))
            .collect()
    }
    /// Return the unique local target selected by a validated placement
    /// heuristic.
    pub fn lookup_host_heuristic(&self, heuristic: &str) -> Option<&ComputeTarget> {
        self.targets
            .iter()
            .find(|target| target.host_heuristic.as_deref() == Some(heuristic))
    }

    /// Return the named coordinator entry.
    pub fn lookup_coordinator(&self, name: &str) -> Option<&Coordinator> {
        self.coordinators.iter().find(|c| c.name == name)
    }
    /// Resolve an operator selector as an exact coordinator name first, then
    /// as its declarative host placement.
    pub fn lookup_coordinator_selector(&self, selector: &str) -> Option<&Coordinator> {
        self.lookup_coordinator(selector).or_else(|| {
            self.coordinators
                .iter()
                .find(|coordinator| coordinator.host_heuristic.as_deref() == Some(selector))
        })
    }

    /// The directory entry for a service, when the registry carries one.
    pub fn service(&self, name: &str) -> Option<&Service> {
        self.service_directory.as_ref()?.services.get(name)
    }

    /// The named placement profile.
    pub fn placement_profile(&self, name: &str) -> Option<&PlacementProfile> {
        self.placement_profiles
            .iter()
            .find(|profile| profile.name == name)
    }

    /// The document as JSON, including every top-level key this build does
    /// not model.
    ///
    /// This is what a writer must serialize: unmodelled top-level keys come
    /// back verbatim out of [`Registry::extra`], so a read-modify-write
    /// through this method keeps the blocks a newer publisher added. The one
    /// thing it does not reproduce is what the loader deliberately drops —
    /// target and coordinator entries with no `name` (Python parity) — and
    /// `targets` / `coordinators` are always emitted, empty if the document
    /// carried none.
    pub fn to_document(&self) -> Value {
        serde_json::to_value(self).expect("registry serialization is infallible")
    }

    /// Find the unique target declaring the normalized host identity.
    ///
    /// Names, explicit hostname aliases, and the host part of legacy SSH
    /// destinations are identities. Ambiguous registry data is rejected
    /// rather than allowing target order to decide which configuration a
    /// host receives.
    pub fn lookup_self(&self, hostname: &str) -> Result<Option<&ComputeTarget>, RegistryError> {
        let identity = normalize_hostname(hostname);
        if identity.is_empty() {
            return Ok(None);
        }
        let mut matches: Vec<&ComputeTarget> = Vec::new();
        for target in &self.targets {
            let mut identities: HashSet<String> = HashSet::new();
            identities.insert(normalize_hostname(&target.name));
            identities.extend(
                target
                    .hostnames
                    .iter()
                    .map(|alias| normalize_hostname(alias)),
            );
            if let Some(ssh) = &target.ssh {
                identities.insert(ssh_hostname(ssh));
            }
            if identities.contains(&identity) {
                matches.push(target);
            }
        }
        if matches.len() > 1 {
            let mut names: Vec<&str> = matches.iter().map(|t| t.name.as_str()).collect();
            names.sort_unstable();
            return Err(RegistryError::AmbiguousIdentity {
                identity,
                names: names.join(", "),
            });
        }
        Ok(matches.into_iter().next())
    }
}
