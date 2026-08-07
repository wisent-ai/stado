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
//! DEVIATION from Python: the fetch follows `WC_STORAGE_BACKEND` instead of
//! hardcoding GCS. Python reads GCS unconditionally, so on an Azure-only
//! deployment the dashboard compare-and-swaps `registry.json` into the
//! Azure container (`dashboard/policy.rs`, the WRITE side, which already
//! goes through the configured store) while every reader consults a GCS
//! object nobody writes. The "gcs" read path is unchanged.
//!
//! [`fetch_registry_remote`] returns [`RegistryFetchError`] rather than an
//! empty registry, because "the store is unreachable" and "the registry
//! does not list you" drive opposite decisions in the coordinator's
//! rogue-daemon kill switch.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

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
    const ALLOWED: [&str; 2] = ["huggingface_cache", "weles_recordings"];
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
    #[serde(default)]
    pub disk_cleanup: Option<DiskCleanupPolicy>,
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
// __init__.py — loaders (local file, GCS fetch with TTL, source selection)
// ---------------------------------------------------------------------------

/// Canonical GCS location of the registry (Python `GCS_REGISTRY_URI`). Only
/// the "gcs" backend resolves the registry here; every other backend reads
/// [`REGISTRY_BLOB`] from the store `config::wc_storage_backend()` selects.
pub const GCS_REGISTRY_URI: &str = "gs://wisent-compute/registry.json";
/// Store-relative path of the registry document, identical on every
/// backend. `dashboard/policy.rs` compare-and-swaps this exact path through
/// the configured store, so the read and write sides address one object.
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

/// A parsed registry document: targets plus coordinator entries.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Registry {
    pub targets: Vec<ComputeTarget>,
    pub coordinators: Vec<Coordinator>,
}

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

/// Parse a registry document from a JSON string.
pub fn load_registry_from_str(text: &str) -> Result<Registry, RegistryError> {
    let data: Value =
        serde_json::from_str(text).map_err(|exc| RegistryError::Json(exc.to_string()))?;
    Ok(Registry {
        targets: parse_targets(&data)?,
        coordinators: parse_coordinators(&data)?,
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

/// Store-relative `registry.json` download: `Ok(Some(text))` = fetched,
/// `Ok(None)` = blob absent (Python `blob.generation is None`), `Err(msg)` =
/// the store could not be reached at all.
pub type RegistryDownloader =
    Arc<dyn Fn() -> BoxFuture<'static, Result<Option<String>, String>> + Send + Sync>;

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
async fn download_registry_blob() -> Result<Option<String>, String> {
    // One seam for both directions: [`RegistryStore`] resolves the same
    // object `cli/registry.rs::push` writes and `dashboard/policy.rs`
    // compare-and-swaps, so a reader can never sit on a dead object while
    // the writer edits a live one.
    let store = RegistryStore::open().await.map_err(|exc| exc.to_string())?;
    store.read_text().await.map_err(|exc| exc.to_string())
}

async fn download_registry() -> Result<Option<String>, String> {
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
        // The `[_load_from_gcs]` prefix is Python's function name, kept
        // verbatim so existing operator log greps still match.
        Ok(Some(text)) => load_registry_from_str(&text).map_err(|source| {
            eprintln!("[_load_from_gcs] failed: {source}");
            RegistryFetchError::Invalid { location, source }
        }),
        // Ok(None) = blob absent (Python `blob.generation is None`).
        Ok(None) => Err(RegistryFetchError::Absent { location }),
        Err(detail) => {
            eprintln!("[_load_from_gcs] failed: {detail}");
            Err(RegistryFetchError::Unreachable { location, detail })
        }
    }
}

/// The registry document, configured store first with the bundled file as
/// fallback (Python `load_targets` / `load_coordinators` with
/// `source="auto"`). Every [`RegistryFetchError`] falls back.
pub async fn load_registry_auto() -> Result<Registry, RegistryError> {
    match fetch_registry_remote().await {
        Ok(registry) => Ok(registry),
        Err(_) => load_bundled_registry(),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_registry_loads() {
        let registry = load_bundled_registry().unwrap();
        assert_eq!(registry.local_targets().len(), registry.targets.len());
        let coordinator = registry.lookup_coordinator("local-control-plane").unwrap();
        assert!(coordinator.active);
        assert_eq!(coordinator.runtime, "daemon");
        assert_eq!(coordinator.state_uri, "stado://system/registry");

        let workstation = registry.lookup("ubuntu-server-rtx-pro-6000").unwrap();
        assert_eq!(workstation.kind, "local");
        assert!(!workstation.pinned_only);

        let mac_mini = registry.lookup("charless-mac-mini").unwrap();
        let cleanup = mac_mini.disk_cleanup.as_ref().unwrap();
        assert!(cleanup.cleaners.contains_key("huggingface_cache"));
        assert_eq!(mac_mini.hostnames, ["charless-mac-mini.local"]);
        assert_eq!(mac_mini.ssh.as_deref(), Some("charles@100.120.25.24"));
    }

    #[test]
    fn bundled_registry_passes_v2_validation() {
        validate_registry_file(&bundled_registry_path()).unwrap();
    }

    #[test]
    fn unknown_keys_land_in_extra() {
        let registry = load_registry_from_str(
            r#"{"targets": [{"name": "box1", "kind": "local", "future_field": {"x": 1}}],
                "coordinators": [{"name": "c1", "custom": true}]}"#,
        )
        .unwrap();
        assert_eq!(
            registry.targets[0].extra["future_field"],
            serde_json::json!({"x": 1})
        );
        assert_eq!(registry.coordinators[0].extra["custom"], Value::Bool(true));
        assert_eq!(registry.coordinators[0].runtime, "daemon");
    }

    #[test]
    fn loader_skips_nameless_and_handles_missing_file() {
        let registry = load_registry_from_str(
            r#"{"targets": [{"kind": "local"}, "not-an-object", {"name": "", "kind": "gcp"},
                {"name": "ok", "kind": "gcp", "hostnames": null}]}"#,
        )
        .unwrap();
        assert_eq!(registry.targets.len(), 1);
        assert_eq!(registry.targets[0].name, "ok");
        assert!(registry.targets[0].hostnames.is_empty());

        let empty = load_registry_file(Path::new("/nonexistent/registry.json")).unwrap();
        assert!(empty.targets.is_empty());
        assert!(empty.coordinators.is_empty());

        assert!(matches!(
            load_registry_from_str("{broken"),
            Err(RegistryError::Json(_))
        ));
    }

    #[test]
    fn hostname_normalization() {
        assert_eq!(normalize_hostname("  Wiscow.LOCAL. "), "wiscow.local");
        assert_eq!(normalize_hostname(""), "");
        assert_eq!(ssh_hostname("charles@100.120.25.24"), "100.120.25.24");
        assert_eq!(ssh_hostname("host.example:2222"), "host.example");
        assert_eq!(ssh_hostname("user@[2001:db8::1]:22"), "2001:db8::1");
        assert_eq!(ssh_hostname("[]"), "");
    }

    #[test]
    fn lookup_self_matches_all_identity_forms() {
        let registry = load_bundled_registry().unwrap();
        assert_eq!(
            registry
                .lookup_self("CHARLESS-MAC-MINI")
                .unwrap()
                .unwrap()
                .name,
            "charless-mac-mini"
        );
        // Explicit hostname alias.
        assert_eq!(
            registry
                .lookup_self("charless-mac-mini.local")
                .unwrap()
                .unwrap()
                .name,
            "charless-mac-mini"
        );
        // Host part of a legacy SSH destination.
        assert_eq!(
            registry.lookup_self("100.120.25.24").unwrap().unwrap().name,
            "charless-mac-mini"
        );
        assert!(registry.lookup_self("no-such-host").unwrap().is_none());
        assert!(registry.lookup_self("  ").unwrap().is_none());
    }

    #[test]
    fn validation_rejects_bad_documents() {
        let err = validate_registry(&serde_json::json!({"schema_version": 1, "targets": []}))
            .unwrap_err();
        assert!(err.0.contains("registry.schema_version"), "{}", err.0);

        let err = validate_registry(&serde_json::json!({
            "schema_version": 2,
            "targets": [
                {"name": "dup", "kind": "gcp"},
                {"name": "dup", "kind": "gcp"},
            ],
        }))
        .unwrap_err();
        assert!(err.0.contains("duplicate target name 'dup'"), "{}", err.0);

        let err = validate_registry(&serde_json::json!({
            "schema_version": 2,
            "targets": [{"name": "ok", "kind": "dcloud"}],
        }))
        .unwrap_err();
        assert!(err.0.contains("['gcp', 'local', 'vast']"), "{}", err.0);

        // weles is only allowed on local targets.
        let err = validate_registry(&serde_json::json!({
            "schema_version": 2,
            "targets": [{"name": "ok", "kind": "gcp", "weles": {"enabled": true, "actions": ["*"]}}],
        }))
        .unwrap_err();
        assert!(err.0.contains("kind='local'"), "{}", err.0);

        // Every grant must be an exact action identifier; wildcard grants are forbidden.
        let err = validate_registry(&serde_json::json!({
            "schema_version": 2,
            "targets": [{"name": "ok", "kind": "local",
                         "weles": {"enabled": true, "actions": ["*", "run"]}}],
        }))
        .unwrap_err();
        assert!(
            err.0.contains("exact lowercase action identifier"),
            "{}",
            err.0
        );

        // Colliding host identities across targets are rejected.
        let err = validate_registry(&serde_json::json!({
            "schema_version": 2,
            "targets": [
                {"name": "one", "kind": "local", "hostnames": ["shared-host"]},
                {"name": "two", "kind": "local", "ssh": "user@shared-host"},
            ],
        }))
        .unwrap_err();
        assert!(err.0.contains("already declared by"), "{}", err.0);

        // disk_cleanup: target_free_gb must exceed low_free_gb.
        let err = validate_registry(&serde_json::json!({
            "schema_version": 2,
            "targets": [{"name": "ok", "kind": "local", "disk_cleanup": {
                "mode": "report", "check_interval_seconds": 300,
                "low_free_gb": 100, "target_free_gb": 50,
                "max_bytes_per_pass": 1048576, "max_items_per_pass": 10,
                "max_scan_items": 100, "cleaners": {},
            }}],
        }))
        .unwrap_err();
        assert!(
            err.0.contains("must be greater than low_free_gb"),
            "{}",
            err.0
        );
    }

    #[test]
    fn capabilities_admission() {
        let box_caps = box_capabilities();
        assert_eq!(box_caps.target_id, "box-linux-sandbox");

        // A default Job fits the box.
        let plain = Job::new("j1", "echo hi");
        let decision = admit_job(&plain, box_caps);
        assert!(decision.accepted, "{:?}", decision.reasons);
        decision.require().unwrap();

        // Any GPU requirement rejects (Python does not consult
        // target.accelerator).
        let mut gpu = Job::new("j2", "train");
        gpu.gpu_mem_gb = 16;
        let decision = admit_job(&gpu, box_caps);
        assert!(!decision.accepted);
        assert_eq!(decision.reasons, ["target has no accelerator"]);

        // Multiple incompatibilities are all reported, in field order.
        let mut heavy = Job::new("j3", "train");
        heavy.platform_os = "Darwin".to_string();
        heavy.cpu_cores = 64;
        heavy.preemptible = true;
        heavy.region = "us-central1".to_string();
        heavy.apt_packages = vec!["htop".to_string()];
        heavy.executor = "other-executor".to_string();
        let decision = admit_job(&heavy, box_caps);
        let err = decision.require().unwrap_err();
        assert_eq!(
            err.0,
            "requires os=darwin, target is linux; \
             requires 64 CPU cores, target has 4; \
             executor 'other-executor' is unsupported; \
             target does not support preemptible lifecycle; \
             target region is not selectable; \
             target does not support provider-managed system packages"
        );
    }

    // ---- registry fetch (loopback mock; serialized via GLOBAL_STATE_LOCK) ----

    /// Install `downloader`, clear the TTL cache, run `f`, restore.
    async fn with_downloader<F, T>(downloader: RegistryDownloader, f: F) -> T
    where
        F: std::future::Future<Output = T>,
    {
        let _guard = crate::testutil::GLOBAL_STATE_LOCK.lock().await;
        set_registry_downloader_for_testing(Some(downloader));
        clear_registry_cache();
        let out = f.await;
        set_registry_downloader_for_testing(None);
        clear_registry_cache();
        out
    }

    #[tokio::test]
    async fn remote_fetch_serves_loopback_mock_and_caches_within_ttl() {
        let body = r#"{"targets": [{"name": "box1", "kind": "local"}],
                        "coordinators": [{"name": "c1", "active": true}]}"#;
        let mock =
            crate::testutil::mock_http(vec![crate::testutil::http_response(200, "OK", body)]).await;
        let requests = Arc::clone(&mock.requests);
        let base_url = mock.base_url.clone();
        with_downloader(
            Arc::new(move || {
                let base_url = base_url.clone();
                Box::pin(async move {
                    let text = reqwest::Client::new()
                        .get(base_url)
                        .timeout(Duration::from_secs(5))
                        .send()
                        .await
                        .map_err(|exc| exc.to_string())?
                        .text()
                        .await
                        .map_err(|exc| exc.to_string())?;
                    Ok(Some(text))
                }) as BoxFuture<'static, Result<Option<String>, String>>
            }),
            async {
                // First call downloads through the loopback mock; the
                // second call within the 30 s TTL must NOT re-download.
                let first = fetch_registry_remote().await.expect("fetch succeeds");
                let second = fetch_registry_remote().await.expect("fetch succeeds");
                assert_eq!(first, second);
                assert_eq!(first.targets[0].name, "box1");
                assert_eq!(first.coordinators[0].name, "c1");
                // load_registry_auto shares the cache with the direct fetch.
                assert_eq!(load_registry_auto().await.unwrap(), first);
                assert_eq!(fetch_registry_remote().await.unwrap(), first);
            },
        )
        .await;
        assert_eq!(
            requests.lock().unwrap().len(),
            1,
            "TTL cache hit must not re-fetch"
        );
        mock.stop();
    }

    #[tokio::test]
    async fn unreadable_registry_falls_back_for_auto_but_errors_for_survival() {
        with_downloader(
            Arc::new(|| {
                Box::pin(async { Err("mock GCS outage".to_string()) })
                    as BoxFuture<'static, Result<Option<String>, String>>
            }),
            async {
                // source="auto": GCS failure falls back to the bundled file.
                let auto = load_registry_auto().await.unwrap();
                assert_eq!(auto, load_bundled_registry().unwrap());
                // The fleet-survival path must see the FAILURE, never an
                // empty registry: "unreachable" is not "you were revoked".
                assert!(matches!(
                    fetch_registry_remote().await,
                    Err(RegistryFetchError::Unreachable { .. })
                ));
            },
        )
        .await;
        with_downloader(
            Arc::new(|| {
                Box::pin(async { Ok(None) }) as BoxFuture<'static, Result<Option<String>, String>>
            }),
            async {
                // Absent blob (Python `blob.generation is None`): a store
                // holding no registry document is equally non-authoritative
                // — an un-seeded Azure container looks exactly like this.
                let auto = load_registry_auto().await.unwrap();
                assert_eq!(auto, load_bundled_registry().unwrap());
                assert!(matches!(
                    fetch_registry_remote().await,
                    Err(RegistryFetchError::Absent { .. })
                ));
            },
        )
        .await;
    }

    #[tokio::test]
    async fn invalid_json_is_a_fetch_failure_not_a_crash() {
        with_downloader(
            Arc::new(|| {
                Box::pin(async { Ok(Some("{broken".to_string())) })
                    as BoxFuture<'static, Result<Option<String>, String>>
            }),
            async {
                assert!(matches!(
                    fetch_registry_remote().await,
                    Err(RegistryFetchError::Invalid { .. })
                ));
                assert_eq!(
                    load_registry_auto().await.unwrap(),
                    load_bundled_registry().unwrap()
                );
            },
        )
        .await;
    }
}
