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
    let ssh_fallbacks = target
        .get("ssh_fallbacks")
        .cloned()
        .map(serde_json::from_value)
        .transpose()
        .map_err(|error| format!("service {service:?} has invalid SSH fallback paths: {error}"))?
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
