//! Declarative, registry-backed service placement groups.
//!
//! A profile names the logical services that move together, the concrete unit
//! installed on each eligible host, the state files that travel, health probes,
//! and routing units whose desired state depends on the selected destination.
//! Runtime transactions are recorded in the same compare-and-swapped registry
//! document so two operators cannot relocate services concurrently.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

const PROFILES_KEY: &str = "placement_profiles";
const TRANSACTIONS_KEY: &str = "placement_transactions";

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlacementProfile {
    pub name: String,
    pub services: Vec<String>,
    pub stop_order: Vec<String>,
    pub start_order: Vec<String>,
    pub state: Vec<PlacementState>,
    pub hosts: BTreeMap<String, PlacementHost>,
    #[serde(default)]
    pub routing: Vec<PlacementRoute>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlacementState {
    /// `$HOME`-relative path. State is never accepted from outside the host home.
    pub path: String,
    #[serde(default)]
    pub required: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlacementHost {
    /// Logical service name -> concrete unit installed on this host.
    pub units: BTreeMap<String, PlacementUnit>,
    pub probes: Vec<PlacementProbe>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlacementUnit {
    /// Name recorded in `targets[].services[]` after cutover.
    pub name: String,
    /// launchd label or systemd unit name.
    pub unit: String,
    /// Absolute unit-file path on the target host.
    pub path: String,
    pub kind: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlacementProbe {
    pub service: String,
    pub url: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlacementRoute {
    /// Host on which the routing unit is installed.
    pub host: String,
    pub unit: PlacementUnit,
    /// The routing unit is enabled only while this is the selected destination.
    pub active_when_destination: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlacementTransaction {
    pub id: String,
    pub profile: String,
    pub from_host: String,
    pub to_host: String,
    pub started_at: String,
}

fn identifier(value: &str) -> bool {
    let bytes = value.as_bytes();
    let edge = |byte: u8| byte.is_ascii_lowercase() || byte.is_ascii_digit();
    !bytes.is_empty()
        && edge(bytes[0])
        && edge(bytes[bytes.len() - 1])
        && bytes
            .iter()
            .all(|byte| edge(*byte) || matches!(byte, b'.' | b'_' | b'-'))
}

fn validate_identifier(value: &str, location: &str) -> Result<(), String> {
    if identifier(value) {
        Ok(())
    } else {
        Err(format!(
            "{location}: must be a lowercase identifier without empty edges"
        ))
    }
}

fn validate_unit(unit: &PlacementUnit, location: &str) -> Result<(), String> {
    validate_identifier(&unit.name, &format!("{location}.name"))?;
    if unit.unit.is_empty() || unit.unit.chars().any(char::is_control) {
        return Err(format!("{location}.unit: must be a non-empty unit name"));
    }
    if unit.kind != "launchd" && unit.kind != "systemd" {
        return Err(format!(
            "{location}.kind: must be one of ['launchd', 'systemd']"
        ));
    }
    let path = Path::new(&unit.path);
    if !path.is_absolute()
        || unit.path.chars().any(char::is_control)
        || path
            .components()
            .any(|component| matches!(component, Component::ParentDir))
    {
        return Err(format!(
            "{location}.path: must be an absolute path without '..'"
        ));
    }
    if unit.kind == "launchd" && path.extension().and_then(|part| part.to_str()) != Some("plist") {
        return Err(format!("{location}.path: launchd units must end in .plist"));
    }
    if unit.kind == "systemd" && !unit.unit.ends_with(".service") {
        return Err(format!(
            "{location}.unit: systemd units must end in .service"
        ));
    }
    Ok(())
}

fn validate_state_path(path: &str, location: &str) -> Result<(), String> {
    let parsed = Path::new(path);
    if path.is_empty()
        || parsed.is_absolute()
        || path.starts_with('~')
        || path.chars().any(char::is_control)
        || parsed.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        return Err(format!("{location}: must be a clean, $HOME-relative path"));
    }
    Ok(())
}

fn validate_probe(probe: &PlacementProbe, location: &str) -> Result<(), String> {
    let parsed = url::Url::parse(&probe.url)
        .map_err(|error| format!("{location}.url: invalid URL: {error}"))?;
    if parsed.scheme() != "http" {
        return Err(format!("{location}.url: must use http on loopback"));
    }
    let loopback = parsed
        .host_str()
        .and_then(|host| host.parse::<std::net::IpAddr>().ok())
        .is_some_and(|address| address.is_loopback());
    if !loopback || parsed.port().is_none() {
        return Err(format!(
            "{location}.url: must use a loopback address and explicit port"
        ));
    }
    Ok(())
}

fn profile_names(document: &Value) -> Result<BTreeSet<String>, String> {
    let targets = document
        .get("targets")
        .and_then(Value::as_array)
        .ok_or_else(|| "registry.targets: must be an array".to_string())?;
    Ok(targets
        .iter()
        .filter_map(|target| target.get("name").and_then(Value::as_str))
        .map(str::to_string)
        .collect())
}

pub fn profiles(document: &Value) -> Result<Vec<PlacementProfile>, String> {
    match document.get(PROFILES_KEY) {
        None => Ok(Vec::new()),
        Some(value) => serde_json::from_value(value.clone())
            .map_err(|error| format!("registry.{PROFILES_KEY}: {error}")),
    }
}

pub fn transactions(document: &Value) -> Result<Vec<PlacementTransaction>, String> {
    match document.get(TRANSACTIONS_KEY) {
        None => Ok(Vec::new()),
        Some(value) => serde_json::from_value(value.clone())
            .map_err(|error| format!("registry.{TRANSACTIONS_KEY}: {error}")),
    }
}

/// Validate optional placement declarations in a registry-v2 document.
pub fn validate_registry_contract(document: &Value) -> Result<(), String> {
    let target_names = profile_names(document)?;
    let profiles = profiles(document)?;
    let mut names = BTreeSet::new();
    for (profile_index, profile) in profiles.iter().enumerate() {
        let location = format!("registry.{PROFILES_KEY}[{profile_index}]");
        validate_identifier(&profile.name, &format!("{location}.name"))?;
        if !names.insert(profile.name.clone()) {
            return Err(format!(
                "{location}.name: duplicate placement profile {:?}",
                profile.name
            ));
        }
        if profile.services.is_empty() {
            return Err(format!("{location}.services: must not be empty"));
        }
        let mut service_names = BTreeSet::new();
        for (index, service) in profile.services.iter().enumerate() {
            validate_identifier(service, &format!("{location}.services[{index}]"))?;
            if !service_names.insert(service.clone()) {
                return Err(format!(
                    "{location}.services[{index}]: duplicate service {service:?}"
                ));
            }
        }
        for (field, order) in [
            ("stop_order", &profile.stop_order),
            ("start_order", &profile.start_order),
        ] {
            let ordered: BTreeSet<&String> = order.iter().collect();
            let expected: BTreeSet<&String> = profile.services.iter().collect();
            if order.len() != profile.services.len() || ordered != expected {
                return Err(format!(
                    "{location}.{field}: must contain every profile service exactly once"
                ));
            }
        }
        if profile.hosts.len() < 2 {
            return Err(format!(
                "{location}.hosts: must contain at least two destinations"
            ));
        }
        for (host, host_profile) in &profile.hosts {
            let host_location = format!("{location}.hosts.{host}");
            if !target_names.contains(host) {
                return Err(format!("{host_location}: host is not a registry target"));
            }
            let configured: BTreeSet<&String> = host_profile.units.keys().collect();
            let expected: BTreeSet<&String> = profile.services.iter().collect();
            if configured != expected {
                return Err(format!(
                    "{host_location}.units: must define every profile service exactly once"
                ));
            }
            for (service, unit) in &host_profile.units {
                validate_unit(unit, &format!("{host_location}.units.{service}"))?;
            }
            let mut probed = BTreeSet::new();
            for (index, probe) in host_profile.probes.iter().enumerate() {
                let probe_location = format!("{host_location}.probes[{index}]");
                if !service_names.contains(&probe.service) {
                    return Err(format!(
                        "{probe_location}.service: is not in the placement profile"
                    ));
                }
                if !probed.insert(&probe.service) {
                    return Err(format!(
                        "{probe_location}.service: duplicate probe for {:?}",
                        probe.service
                    ));
                }
                validate_probe(probe, &probe_location)?;
            }
            if probed.len() != service_names.len() {
                return Err(format!(
                    "{host_location}.probes: must probe every profile service"
                ));
            }
        }
        let mut state_paths = BTreeSet::new();
        if profile.state.is_empty() {
            return Err(format!("{location}.state: must not be empty"));
        }
        for (index, state) in profile.state.iter().enumerate() {
            validate_state_path(&state.path, &format!("{location}.state[{index}].path"))?;
            if !state_paths.insert(&state.path) {
                return Err(format!(
                    "{location}.state[{index}].path: duplicate state path {:?}",
                    state.path
                ));
            }
        }
        for (index, route) in profile.routing.iter().enumerate() {
            let route_location = format!("{location}.routing[{index}]");
            if !profile.hosts.contains_key(&route.host) {
                return Err(format!(
                    "{route_location}.host: must be one of the profile hosts"
                ));
            }
            if !profile.hosts.contains_key(&route.active_when_destination) {
                return Err(format!(
                    "{route_location}.active_when_destination: must be one of the profile hosts"
                ));
            }
            validate_unit(&route.unit, &format!("{route_location}.unit"))?;
        }
    }

    let transactions = transactions(document)?;
    let mut transaction_profiles = BTreeSet::new();
    for (index, transaction) in transactions.iter().enumerate() {
        let location = format!("registry.{TRANSACTIONS_KEY}[{index}]");
        if uuid::Uuid::parse_str(&transaction.id).is_err() {
            return Err(format!("{location}.id: must be a UUID"));
        }
        if !names.contains(&transaction.profile) {
            return Err(format!(
                "{location}.profile: references an unknown placement profile"
            ));
        }
        if !target_names.contains(&transaction.from_host)
            || !target_names.contains(&transaction.to_host)
            || transaction.from_host == transaction.to_host
        {
            return Err(format!(
                "{location}: must reference two different registry targets"
            ));
        }
        if chrono::DateTime::parse_from_rfc3339(&transaction.started_at).is_err() {
            return Err(format!("{location}.started_at: must be RFC3339"));
        }
        if !transaction_profiles.insert(&transaction.profile) {
            return Err(format!(
                "{location}.profile: another transaction already owns this profile"
            ));
        }
    }
    Ok(())
}

pub fn profile_for_services(
    document: &Value,
    requested: &[String],
) -> Result<PlacementProfile, String> {
    if requested.is_empty() {
        return Err("placement move requires at least one service".to_string());
    }
    let requested_set: BTreeSet<&String> = requested.iter().collect();
    if requested_set.len() != requested.len() {
        return Err("placement move service names must not repeat".to_string());
    }
    let matches: Vec<PlacementProfile> = profiles(document)?
        .into_iter()
        .filter(|profile| {
            profile.services.len() == requested.len()
                && profile.services.iter().collect::<BTreeSet<_>>() == requested_set
        })
        .collect();
    match matches.as_slice() {
        [profile] => Ok(profile.clone()),
        [] => Err(format!(
            "no placement profile matches services {}",
            requested.join(" ")
        )),
        _ => Err(format!(
            "services {} match multiple placement profiles",
            requested.join(" ")
        )),
    }
}

pub fn claim_transaction(
    document: &mut Value,
    transaction: &PlacementTransaction,
) -> Result<(), String> {
    let root = document
        .as_object_mut()
        .ok_or_else(|| "registry: must be an object".to_string())?;
    let active = root
        .entry(TRANSACTIONS_KEY.to_string())
        .or_insert_with(|| Value::Array(Vec::new()))
        .as_array_mut()
        .ok_or_else(|| format!("registry.{TRANSACTIONS_KEY}: must be an array"))?;
    if !active.is_empty() {
        return Err(format!(
            "another placement transaction is active: {}",
            active
                .iter()
                .filter_map(|value| value.get("id").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    active.push(
        serde_json::to_value(transaction)
            .map_err(|error| format!("could not serialize placement transaction: {error}"))?,
    );
    Ok(())
}

pub fn release_transaction(document: &mut Value, id: &str) -> Result<bool, String> {
    let root = document
        .as_object_mut()
        .ok_or_else(|| "registry: must be an object".to_string())?;
    let Some(active) = root.get_mut(TRANSACTIONS_KEY) else {
        return Ok(false);
    };
    let active = active
        .as_array_mut()
        .ok_or_else(|| format!("registry.{TRANSACTIONS_KEY}: must be an array"))?;
    let previous_len = active.len();
    active.retain(|value| value.get("id").and_then(Value::as_str) != Some(id));
    let removed = active.len() != previous_len;
    if active.is_empty() {
        root.remove(TRANSACTIONS_KEY);
    }
    Ok(removed)
}

pub fn root_object(document: &mut Value) -> Result<&mut Map<String, Value>, String> {
    document
        .as_object_mut()
        .ok_or_else(|| "registry: must be an object".to_string())
}
