//! Registry-backed logical service resolution.
//!
//! Workloads name `stado://service/<name>`. Physical hosts, loopback ports,
//! and SSH transport remain registry-owned details consumed only by the local
//! Stado resolver. A monotonically increasing directory generation makes a
//! placement cutover one atomic fleet-visible change.
//!
//! The shapes a directory is made of are in `types`, the checks a registry
//! has to pass are in `validate`, and the one defect that makes a resolver
//! answer with its own socket is in `self_reference`. What is left here is
//! reading a directory and resolving one service through it.

use serde_json::Value;

use validate::targets;

mod self_reference;
mod types;
mod validate;

pub use self_reference::{self_referencing_endpoints, SelfReferencingEndpoint};
pub use types::{
    ResolvedService, ResolverAdapter, ResolverConfig, ServiceAuthority, ServiceConsumer,
    ServiceDirectory, ServiceEndpoint, ServiceRoute,
};
pub use validate::validate_registry_contract;

const DIRECTORY_KEY: &str = "service_directory";

/// The scheme a configuration value uses to name a service instead of an
/// address: `stado://service/<name>?consumer=<consumer>`.
const SERVICE_SCHEME: &str = "stado://service/";

/// A configured address, with a named service resolved on this host.
///
/// A value that names `stado://service/<name>?consumer=<consumer>` is the
/// address this host's resolver published for that adapter, read when it is
/// used, so the configuration keeps no copy of a port that the host handed
/// out and may hand out again. An adapter no listening resolver published
/// answers empty, which every reader refuses with its own sentence. Any other
/// value is returned as it is.
pub fn local_address(value: &str) -> String {
    let Some(named) = value.trim().strip_prefix(SERVICE_SCHEME) else {
        return value.to_string();
    };
    let (service, consumer) = match named.split_once("?consumer=") {
        Some((service, consumer)) => (service, consumer),
        None => (named, ""),
    };
    if service.is_empty() || consumer.is_empty() {
        eprintln!(
            "stado: {value:?} names no consumer; write {SERVICE_SCHEME}<service>?consumer=<consumer>"
        );
        return String::new();
    }
    crate::cli::resolver::published_adapter_url(service, consumer).unwrap_or_else(|| {
        eprintln!(
            "stado: no listening resolver on this host published an adapter for {service} as \
             consumer {consumer}; `stado resolver status` names the adapters it serves"
        );
        String::new()
    })
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

fn profile_is_locked(document: &Value, profile: &str) -> Result<bool, String> {
    Ok(crate::placement::transactions(document)?
        .iter()
        .any(|transaction| transaction.profile == profile))
}

/// Why a consumer gets no route to a service. Each variant is a different
/// answer for the caller: a directory that is not the declaration it must be,
/// a service nobody declared, a consumer the service does not admit, and a
/// service held by a placement move that a later call can reach.
#[derive(Debug)]
pub enum ResolveError {
    /// The directory, or a record resolution reads, is malformed or absent.
    Declaration(String),
    /// The directory declares no service by this name.
    UnknownService(String),
    /// The service does not admit this consumer.
    Unauthorized { service: String, consumer: String },
    /// A placement transaction holds the service's profile.
    Moving { service: String, profile: String },
}

impl std::fmt::Display for ResolveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Declaration(detail) => f.write_str(detail),
            Self::UnknownService(service) => write!(f, "unknown logical service {service:?}"),
            Self::Unauthorized { service, consumer } => write!(
                f,
                "consumer {consumer:?} is not authorized for service {service:?}"
            ),
            Self::Moving { service, profile } => write!(
                f,
                "service {service:?} is unavailable during placement transaction for {profile:?}"
            ),
        }
    }
}

impl std::error::Error for ResolveError {}

impl From<ResolveError> for String {
    fn from(error: ResolveError) -> Self {
        error.to_string()
    }
}

/// Resolve a logical service for one workload identity. Resolution fails while
/// the owning placement profile is being moved, preventing a partially staged
/// destination from receiving traffic.
pub fn resolve(
    document: &Value,
    service: &str,
    consumer: &str,
) -> Result<ResolvedService, ResolveError> {
    let directory = directory(document)
        .map_err(ResolveError::Declaration)?
        .ok_or_else(|| {
            ResolveError::Declaration(
                "registry.service_directory: is required for resolution".to_string(),
            )
        })?;
    let route = directory
        .services
        .get(service)
        .ok_or_else(|| ResolveError::UnknownService(service.to_string()))?;
    let policy = route
        .consumers
        .get(consumer)
        .ok_or_else(|| ResolveError::Unauthorized {
            service: service.to_string(),
            consumer: consumer.to_string(),
        })?;
    if let Some(profile) = &route.placement_profile {
        if profile_is_locked(document, profile).map_err(ResolveError::Declaration)? {
            return Err(ResolveError::Moving {
                service: service.to_string(),
                profile: profile.clone(),
            });
        }
    }
    let endpoint = route
        .endpoints
        .get(&route.active_host)
        .cloned()
        .ok_or_else(|| {
            ResolveError::Declaration(format!(
                "service {service:?} has no endpoint on its active host"
            ))
        })?;
    let target_entries = targets(document).map_err(ResolveError::Declaration)?;
    let target = target_entries
        .get(&route.active_host)
        .copied()
        .ok_or_else(|| {
            ResolveError::Declaration(format!(
                "service {service:?} references an unknown active host"
            ))
        })?;
    let ssh = target
        .get("ssh")
        .and_then(Value::as_str)
        .map(str::to_string);
    let ssh_fallbacks = target
        .get("ssh_fallbacks")
        .cloned()
        .map(serde_json::from_value)
        .transpose()
        .map_err(|error| {
            ResolveError::Declaration(format!(
                "service {service:?} has invalid SSH fallback paths: {error}"
            ))
        })?
        .unwrap_or_default();
    Ok(ResolvedService {
        name: service.to_string(),
        generation: directory.generation,
        active_host: route.active_host.clone(),
        endpoint,
        ssh,
        ssh_fallbacks,
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

/// Advance the directory's publication counter and return the new value.
///
/// [`ServiceDirectory::generation`] is the number a consumer compares against
/// the copy it cached, so it detects a stale cache only if EVERY writer that
/// changes the directory advances it. It was advanced by hand in two places
/// and not at all by `service declare`, `service retire` and `service
/// remove`, which add and drop `services.<name>` entries — so a consumer
/// could hold a directory that had already changed beneath it and read a
/// generation saying it had not.
pub fn advance_generation(document: &mut Value) -> Result<u64, String> {
    let block = document
        .get_mut(DIRECTORY_KEY)
        .and_then(Value::as_object_mut)
        .ok_or_else(|| format!("registry.{DIRECTORY_KEY}: is not an object"))?;
    let current = block
        .get("generation")
        .and_then(Value::as_u64)
        .ok_or_else(|| format!("registry.{DIRECTORY_KEY}.generation is not an unsigned integer"))?;
    let next = current
        .checked_add(1)
        .ok_or_else(|| format!("registry.{DIRECTORY_KEY}.generation overflow"))?;
    block.insert("generation".to_string(), Value::from(next));
    Ok(next)
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
