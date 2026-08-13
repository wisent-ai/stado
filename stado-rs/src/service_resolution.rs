//! Registry-backed logical service resolution.
//!
//! Workloads name `stado://service/<name>`. Physical hosts, loopback ports,
//! and SSH transport remain registry-owned details consumed only by the local
//! Stado resolver. A monotonically increasing directory generation makes a
//! placement cutover one atomic fleet-visible change.

use std::collections::{BTreeMap, BTreeSet};
use std::net::IpAddr;
use std::path::{Component, Path};

use serde::{Deserialize, Serialize};
use serde_json::Value;

const DIRECTORY_KEY: &str = "service_directory";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServiceDirectory {
    pub authority: ServiceAuthority,
    pub generation: u64,
    pub services: BTreeMap<String, ServiceRoute>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServiceAuthority {
    pub target: String,
    /// Absolute Stado executable on the authority host.
    pub command: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServiceRoute {
    #[serde(default)]
    pub placement_profile: Option<String>,
    /// Exact managed-service declaration for a fixed service. Placement-backed
    /// services derive this from their profile and must leave it absent.
    #[serde(default)]
    pub managed_service: Option<String>,
    pub active_host: String,
    pub endpoints: BTreeMap<String, ServiceEndpoint>,
    /// Addresses hosts would serve on if the service moved to them, never
    /// addresses to call ([`crate::targets::Service::standby`]).
    ///
    /// Nothing in this module resolves through it — a resolver hands out the
    /// active host's endpoint and a standby address is by construction not
    /// serving. It is modelled here for the asymmetry recorded on `verify`
    /// below: this reader denies unknown keys where `targets::Service`
    /// tolerates them, so a field added on the tolerant side alone takes
    /// every resolver on the fleet down the moment one directory entry is
    /// published carrying it.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub standby: BTreeMap<String, ServiceEndpoint>,
    pub consumers: BTreeMap<String, ServiceConsumer>,
    /// How this route is checked against the world
    /// ([`crate::targets::Service::verification`] derives the default when it
    /// is absent).
    ///
    /// Modelled here because two readers parse this same entry with opposite
    /// strictness: `targets::Service` keeps unmodelled keys in a
    /// `serde(flatten)` `extra`, this one denies them outright. A field added
    /// to satisfy the tolerant reader alone would take the resolver down
    /// fleet-wide the moment it was published — every host refusing the whole
    /// directory over a key it merely did not know. Any future field on a
    /// service entry has to land in both places, in the same change.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verify: Option<crate::targets::VerifyDescriptor>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServiceEndpoint {
    /// Host-relative base URL. Loopback means loopback on `active_host`, not
    /// on the workload host.
    pub url: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServiceConsumer {
    #[serde(default)]
    pub capabilities: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResolverConfig {
    pub api_bind: String,
    #[serde(default = "default_refresh_seconds")]
    pub refresh_seconds: u64,
    #[serde(default = "default_max_stale_seconds")]
    pub max_stale_seconds: u64,
    pub adapters: Vec<ResolverAdapter>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResolverAdapter {
    pub service: String,
    pub bind: String,
    pub consumer: String,
    #[serde(default = "default_adapter_idle_seconds")]
    pub idle_seconds: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedService {
    pub name: String,
    pub generation: u64,
    pub active_host: String,
    pub endpoint: ServiceEndpoint,
    pub ssh: Option<String>,
    pub capabilities: Vec<String>,
}

fn default_refresh_seconds() -> u64 {
    5
}

fn default_max_stale_seconds() -> u64 {
    60
}

/// Short enough that retained sockets stay bounded: two directory-freshness
/// windows.
///
/// A request/response connection sends nothing in either direction while the
/// service works, so this window is also a cap on how long a proxied service may
/// take to answer. Model dispatch legitimately exceeds two minutes, and raising
/// this default to cover it tripled retention for every adapter on the fleet --
/// which exhausted the resolver's file descriptors and took the whole local data
/// plane down with `Too many open files`. The long window belongs on the
/// adapters that need it, declared per adapter in the registry, not on
/// everything.
fn default_adapter_idle_seconds() -> u64 {
    default_max_stale_seconds().saturating_add(default_max_stale_seconds())
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

pub fn directory(document: &Value) -> Result<Option<ServiceDirectory>, String> {
    document
        .get(DIRECTORY_KEY)
        .map(|value| {
            serde_json::from_value(value.clone())
                .map_err(|error| format!("registry.{DIRECTORY_KEY}: {error}"))
        })
        .transpose()
}

fn targets(document: &Value) -> Result<BTreeMap<String, &Value>, String> {
    let entries = document
        .get("targets")
        .and_then(Value::as_array)
        .ok_or_else(|| "registry.targets: must be an array".to_string())?;
    Ok(entries
        .iter()
        .filter_map(|entry| {
            entry
                .get("name")
                .and_then(Value::as_str)
                .map(|name| (name.to_string(), entry))
        })
        .collect())
}

fn target_declares_service(target: &Value, service: &str) -> bool {
    target
        .get("services")
        .and_then(Value::as_array)
        .is_some_and(|services| {
            services
                .iter()
                .any(|entry| entry.get("name").and_then(Value::as_str) == Some(service))
        })
}

fn active_profile_host(
    profile: &crate::placement::PlacementProfile,
    service: &str,
    target_entries: &BTreeMap<String, &Value>,
) -> Result<String, String> {
    let mut active = Vec::new();
    for (host, host_profile) in &profile.hosts {
        let unit = host_profile.units.get(service).ok_or_else(|| {
            format!(
                "placement profile {:?} has no unit for service {service:?} on {host:?}",
                profile.name
            )
        })?;
        if target_entries
            .get(host)
            .is_some_and(|target| target_declares_service(target, &unit.name))
        {
            active.push(host.clone());
        }
    }
    match active.as_slice() {
        [host] => Ok(host.clone()),
        [] => Err(format!(
            "placement profile {:?} service {service:?} has no active managed unit",
            profile.name
        )),
        _ => Err(format!(
            "placement profile {:?} service {service:?} has multiple active managed units: {}",
            profile.name,
            active.join(", ")
        )),
    }
}

fn validate_endpoint(endpoint: &ServiceEndpoint, location: &str) -> Result<(), String> {
    let url = url::Url::parse(&endpoint.url)
        .map_err(|error| format!("{location}.url: invalid URL: {error}"))?;
    if url.scheme() != "http" {
        return Err(format!(
            "{location}.url: must use host-relative loopback HTTP"
        ));
    }
    if url.username() != ""
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(format!(
            "{location}.url: must not contain credentials, query, or fragment"
        ));
    }
    if url.path() != "/" && !url.path().is_empty() {
        return Err(format!("{location}.url: must be an origin without a path"));
    }
    let loopback = url
        .host_str()
        .and_then(|host| host.parse::<IpAddr>().ok())
        .is_some_and(|address| address.is_loopback());
    if !loopback || url.port_or_known_default().is_none() {
        return Err(format!(
            "{location}.url: must use host-relative loopback with a known port"
        ));
    }
    Ok(())
}

fn validate_resolver_config(
    target_name: &str,
    target: &Value,
    directory: &ServiceDirectory,
) -> Result<(), String> {
    let Some(value) = target.get("service_resolver") else {
        return Ok(());
    };
    let location = format!("registry.targets[{target_name}].service_resolver");
    let config: ResolverConfig =
        serde_json::from_value(value.clone()).map_err(|error| format!("{location}: {error}"))?;
    if config.refresh_seconds == 0 {
        return Err(format!("{location}.refresh_seconds: must be positive"));
    }
    if config.max_stale_seconds < config.refresh_seconds {
        return Err(format!(
            "{location}.max_stale_seconds: must not be shorter than refresh_seconds"
        ));
    }
    let api: std::net::SocketAddr = config
        .api_bind
        .parse()
        .map_err(|_| format!("{location}.api_bind: must be an IP socket address"))?;
    if !api.ip().is_loopback() {
        return Err(format!("{location}.api_bind: must be loopback"));
    }
    let mut binds = BTreeSet::from([config.api_bind.clone()]);
    for (index, adapter) in config.adapters.iter().enumerate() {
        let adapter_location = format!("{location}.adapters[{index}]");
        validate_identifier(&adapter.service, &format!("{adapter_location}.service"))?;
        validate_identifier(&adapter.consumer, &format!("{adapter_location}.consumer"))?;
        if adapter.idle_seconds == 0 {
            return Err(format!("{adapter_location}.idle_seconds: must be positive"));
        }
        let bind: std::net::SocketAddr = adapter
            .bind
            .parse()
            .map_err(|_| format!("{adapter_location}.bind: must be an IP socket address"))?;
        if !bind.ip().is_loopback() {
            return Err(format!("{adapter_location}.bind: must be loopback"));
        }
        if !binds.insert(adapter.bind.clone()) {
            return Err(format!("{adapter_location}.bind: duplicate resolver bind"));
        }
        let route = directory
            .services
            .get(&adapter.service)
            .ok_or_else(|| format!("{adapter_location}.service: unknown logical service"))?;
        if !route.consumers.contains_key(&adapter.consumer) {
            return Err(format!(
                "{adapter_location}.consumer: is not authorized by the service route"
            ));
        }
    }
    Ok(())
}

/// Validate the optional logical service directory and per-host resolver
/// configuration. Registries predating the directory stay readable, but the
/// resolver itself refuses to start without it.
pub fn validate_registry_contract(document: &Value) -> Result<(), String> {
    let Some(directory) = directory(document)? else {
        return Ok(());
    };
    if directory.generation == 0 {
        return Err(format!(
            "registry.{DIRECTORY_KEY}.generation: must be positive"
        ));
    }
    if directory.services.is_empty() {
        return Err(format!(
            "registry.{DIRECTORY_KEY}.services: must not be empty"
        ));
    }
    let target_entries = targets(document)?;
    if !target_entries.contains_key(&directory.authority.target) {
        return Err(format!(
            "registry.{DIRECTORY_KEY}.authority.target: unknown registry target"
        ));
    }
    let command = Path::new(&directory.authority.command);
    if !command.is_absolute()
        || directory.authority.command.chars().any(char::is_control)
        || command
            .components()
            .any(|component| matches!(component, Component::ParentDir))
    {
        return Err(format!(
            "registry.{DIRECTORY_KEY}.authority.command: must be an absolute path without '..'"
        ));
    }
    if target_entries
        .get(&directory.authority.target)
        .and_then(|target| target.get("ssh"))
        .and_then(Value::as_str)
        .is_none()
    {
        return Err(format!(
            "registry.{DIRECTORY_KEY}.authority.target: must declare SSH transport"
        ));
    }
    let profiles = crate::placement::profiles(document)?;
    let profiles: BTreeMap<_, _> = profiles
        .into_iter()
        .map(|profile| (profile.name.clone(), profile))
        .collect();
    for (name, route) in &directory.services {
        let location = format!("registry.{DIRECTORY_KEY}.services.{name}");
        validate_identifier(name, &location)?;
        // Only the explicit descriptor is checked: the derived default is
        // valid by construction, and a registry written before the field
        // existed must not start failing validation because a newer build can
        // now name a thing it does not declare.
        if let Some(verify) = route.verify.as_ref() {
            let problems = crate::targets::validate_verification(&location, verify);
            if !problems.is_empty() {
                return Err(problems.join("; "));
            }
        }
        if !target_entries.contains_key(&route.active_host) {
            return Err(format!("{location}.active_host: unknown registry target"));
        }
        if route.endpoints.is_empty() {
            return Err(format!("{location}.endpoints: must not be empty"));
        }
        if !route.endpoints.contains_key(&route.active_host) {
            return Err(format!("{location}.active_host: has no declared endpoint"));
        }
        for (host, endpoint) in &route.endpoints {
            if !target_entries.contains_key(host) {
                return Err(format!(
                    "{location}.endpoints.{host}: unknown registry target"
                ));
            }
            validate_endpoint(endpoint, &format!("{location}.endpoints.{host}"))?;
            if target_entries
                .get(host)
                .and_then(|target| target.get("ssh"))
                .and_then(Value::as_str)
                .is_none()
            {
                return Err(format!(
                    "{location}.endpoints.{host}: remote resolution requires targets[].ssh"
                ));
            }
        }
        // A standby address is checked for shape and for naming a real host,
        // and for nothing else. It must be a host-relative loopback origin
        // like every other address here, because the day it is promoted it
        // becomes an `endpoints` entry unchanged; but no `ssh` transport is
        // demanded of its host, since nothing resolves through it while the
        // service is elsewhere. A declaration nobody validates is how the
        // wrong port reaches a forward file, so it is validated where it is
        // written rather than where it is dialled.
        for (host, endpoint) in &route.standby {
            if !target_entries.contains_key(host) {
                return Err(format!(
                    "{location}.standby.{host}: unknown registry target"
                ));
            }
            if host == &route.active_host {
                return Err(format!(
                    "{location}.standby.{host}: is the active host, which serves \
                     rather than stands by"
                ));
            }
            validate_endpoint(endpoint, &format!("{location}.standby.{host}"))?;
        }
        if route.consumers.is_empty() {
            return Err(format!("{location}.consumers: must not be empty"));
        }
        for (consumer, policy) in &route.consumers {
            validate_identifier(consumer, &format!("{location}.consumers.{consumer}"))?;
            let mut capabilities = BTreeSet::new();
            for (index, capability) in policy.capabilities.iter().enumerate() {
                validate_identifier(
                    capability,
                    &format!("{location}.consumers.{consumer}.capabilities[{index}]"),
                )?;
                if !capabilities.insert(capability) {
                    return Err(format!(
                        "{location}.consumers.{consumer}.capabilities[{index}]: duplicate capability"
                    ));
                }
            }
        }
        if let Some(profile_name) = &route.placement_profile {
            if route.managed_service.is_some() {
                return Err(format!(
                    "{location}.managed_service: placement-backed routes derive their managed unit from the profile"
                ));
            }
            let profile = profiles.get(profile_name).ok_or_else(|| {
                format!("{location}.placement_profile: unknown placement profile")
            })?;
            if !profile.services.iter().any(|service| service == name) {
                return Err(format!(
                    "{location}.placement_profile: profile does not contain this service"
                ));
            }
            // A placement host is named by one map or the other: `endpoints`
            // if it calls the service, `standby` if it holds the address it
            // would serve on after the move. Before those were two fields the
            // coverage rule could read `endpoints` alone; requiring that now
            // would refuse the whole document the first time a standby
            // address is filed where it belongs, which is the same fleet-wide
            // refusal the `standby` field itself is here to avoid. What must
            // not happen is a placement host with no address anywhere: the
            // cutover then moves the service to a machine nothing can name.
            let expected: BTreeSet<_> = profile.hosts.keys().cloned().collect();
            let declared: BTreeSet<_> = route
                .endpoints
                .keys()
                .chain(route.standby.keys())
                .cloned()
                .collect();
            if declared != expected {
                return Err(format!(
                    "{location}: endpoints and standby together must name every \
                     placement host exactly once"
                ));
            }
            let declared_host = active_profile_host(profile, name, &target_entries)?;
            if route.active_host != declared_host {
                return Err(format!(
                    "{location}.active_host: must match the managed unit on {declared_host:?}"
                ));
            }
        } else {
            let managed_service = route.managed_service.as_deref().ok_or_else(|| {
                format!("{location}.managed_service: fixed routes must name their managed service")
            })?;
            validate_identifier(managed_service, &format!("{location}.managed_service"))?;
            let target = target_entries
                .get(&route.active_host)
                .copied()
                .ok_or_else(|| format!("{location}.active_host: unknown registry target"))?;
            if !target_declares_service(target, managed_service) {
                return Err(format!(
                    "{location}.managed_service: is not declared on the active host"
                ));
            }
        }
    }
    for (profile_name, profile) in &profiles {
        for service in &profile.services {
            let route = directory.services.get(service).ok_or_else(|| {
                format!(
                    "registry.{DIRECTORY_KEY}.services: placement profile {profile_name:?} is missing service {service:?}"
                )
            })?;
            if route.placement_profile.as_deref() != Some(profile_name) {
                return Err(format!(
                    "registry.{DIRECTORY_KEY}.services.{service}.placement_profile: must be {profile_name:?}"
                ));
            }
        }
    }
    for (target_name, target) in &target_entries {
        validate_resolver_config(target_name, target, &directory)?;
    }
    Ok(())
}

fn profile_is_locked(document: &Value, profile: &str) -> Result<bool, String> {
    Ok(crate::placement::transactions(document)?
        .iter()
        .any(|transaction| transaction.profile == profile))
}

/// Resolve a logical service for one workload identity. Resolution fails while
/// the owning placement profile is being moved, preventing a partially staged
/// destination from receiving traffic.
pub fn resolve(document: &Value, service: &str, consumer: &str) -> Result<ResolvedService, String> {
    let directory = directory(document)?
        .ok_or_else(|| "registry.service_directory: is required for resolution".to_string())?;
    let route = directory
        .services
        .get(service)
        .ok_or_else(|| format!("unknown logical service {service:?}"))?;
    let policy = route.consumers.get(consumer).ok_or_else(|| {
        format!("consumer {consumer:?} is not authorized for service {service:?}")
    })?;
    if let Some(profile) = &route.placement_profile {
        if profile_is_locked(document, profile)? {
            return Err(format!(
                "service {service:?} is unavailable during placement transaction for {profile:?}"
            ));
        }
    }
    let endpoint = route
        .endpoints
        .get(&route.active_host)
        .cloned()
        .ok_or_else(|| format!("service {service:?} has no endpoint on its active host"))?;
    let target_entries = targets(document)?;
    let target = target_entries
        .get(&route.active_host)
        .copied()
        .ok_or_else(|| format!("service {service:?} references an unknown active host"))?;
    let ssh = target
        .get("ssh")
        .and_then(Value::as_str)
        .map(str::to_string);
    Ok(ResolvedService {
        name: service.to_string(),
        generation: directory.generation,
        active_host: route.active_host.clone(),
        endpoint,
        ssh,
        capabilities: policy.capabilities.clone(),
    })
}

/// Atomically publish a placement cutover in the same registry document that
/// moves the service units. The directory generation advances once no matter
/// how many services belong to the profile.
pub fn retarget_profile(
    document: &mut Value,
    profile: &str,
    destination: &str,
) -> Result<bool, String> {
    let Some(value) = document.get_mut(DIRECTORY_KEY) else {
        return Ok(false);
    };
    let mut directory: ServiceDirectory = serde_json::from_value(value.clone())
        .map_err(|error| format!("registry.{DIRECTORY_KEY}: {error}"))?;
    let mut changed = false;
    for route in directory.services.values_mut() {
        if route.placement_profile.as_deref() != Some(profile) {
            continue;
        }
        if !route.endpoints.contains_key(destination) {
            return Err(format!(
                "service route for profile {profile:?} has no endpoint on {destination:?}"
            ));
        }
        if route.active_host != destination {
            route.active_host = destination.to_string();
            changed = true;
        }
    }
    if changed {
        directory.generation = directory
            .generation
            .checked_add(1)
            .ok_or_else(|| "registry.service_directory.generation overflow".to_string())?;
        *value = serde_json::to_value(directory)
            .map_err(|error| format!("could not serialize service directory: {error}"))?;
    }
    Ok(changed)
}

pub fn resolver_config(document: &Value, target: &str) -> Result<ResolverConfig, String> {
    let target_entries = targets(document)?;
    let target = target_entries
        .get(target)
        .copied()
        .ok_or_else(|| format!("resolver target {target:?} is not registered"))?;
    let value = target
        .get("service_resolver")
        .ok_or_else(|| "registry target has no service_resolver configuration".to_string())?;
    serde_json::from_value(value.clone())
        .map_err(|error| format!("registry target service_resolver: {error}"))
}
