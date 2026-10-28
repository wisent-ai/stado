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

mod error;
mod managed_unit;
mod self_reference;
mod types;
mod validate;

pub use error::ResolveError;
pub use managed_unit::{managed_unit_on, ManagedUnitRefusal};
pub use self_reference::{listener_address, self_referencing_endpoints, SelfReferencingEndpoint};
pub use types::{
    ResolvedService, ResolverAdapter, ResolverConfig, ServiceAuthority, ServiceConsumer,
    ServiceDirectory, ServiceEndpoint, ServiceRoute,
};
pub use validate::validate_registry_contract;

const DIRECTORY_KEY: &str = "service_directory";

/// How far one change of the directory advances its generation.
const GENERATION_STEP: u64 = 1; // https://stado.wisent.com/docs/primitives/directory ("every registry writer that changes the directory advances it by one")

/// The scheme a configuration value uses to name a service instead of an
/// address: `stado://service/<name>?consumer=<consumer>`.
pub const SERVICE_SCHEME: &str = "stado://service/";

/// A configured address, with a named service resolved on this host.
///
/// A value that names `stado://service/<name>?consumer=<consumer>` is the
/// address this host's resolver published for that adapter, read when it is
/// used, so the configuration keeps no copy of a port that the host handed
/// out and may hand out again. While no listening resolver has published it
/// (the process that runs the resolver is still starting), it is the address
/// the registry declares for that adapter on this host, read from the
/// last-known-good copy: the same number the resolver is about to bind, so a
/// reader started beside it gets a connection error to report rather than an
/// empty URL that ends its process. An adapter neither published nor
/// declared, or a value naming no consumer, is refused with its cause, so
/// the reader says why instead of calling an empty address invalid. Any
/// other value is returned as it is.
pub fn local_address(value: &str) -> Result<String, String> {
    let Some(named) = value.trim().strip_prefix(SERVICE_SCHEME) else {
        return Ok(value.to_string());
    };
    let (service, consumer) = match named.split_once("?consumer=") {
        Some((service, consumer)) => (service, consumer),
        None => (named, ""),
    };
    if service.is_empty() || consumer.is_empty() {
        return Err(format!(
            "{value:?} names no consumer; write {SERVICE_SCHEME}<service>?consumer=<consumer>"
        ));
    }
    if let Some(url) = crate::cli::resolver::published_adapter_url(service, consumer) {
        return Ok(url);
    }
    declared_adapter_url(service, consumer).map_err(|detail| {
        format!(
            "{value}: no listening resolver on this host published an adapter for {service} as \
             consumer {consumer}, and the registry copy does not declare one: {detail}; \
             `stado resolver status` names the adapters this host serves"
        )
    })
}

/// `http://<bind>` the last-known-good registry declares for this host's
/// adapter for `service` and `consumer`.
fn declared_adapter_url(service: &str, consumer: &str) -> Result<String, String> {
    let document = crate::cli::resolver::last_good_document()?;
    let target = crate::cli::resolver::current_target(&document)?;
    resolver_config(&document, &target)?
        .adapters
        .into_iter()
        .find(|adapter| adapter.service == service && adapter.consumer == consumer)
        .map(|adapter| format!("http://{}", adapter.bind))
        .ok_or_else(|| format!("{target} declares no {service} adapter for {consumer}"))
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

/// The placement transaction holding `profile`, if one does.
fn holding_transaction(
    document: &Value,
    profile: &str,
) -> Result<Option<crate::placement::PlacementTransaction>, String> {
    Ok(crate::placement::transactions(document)?
        .into_iter()
        .find(|transaction| transaction.profile == profile))
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
        if let Some(transaction) =
            holding_transaction(document, profile).map_err(ResolveError::Declaration)?
        {
            return Err(ResolveError::Moving {
                service: service.to_string(),
                transaction: Box::new(transaction),
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
    // A target that declares no ssh_fallbacks has none: the registry states
    // its primary SSH path as the only one. A list that does not parse is
    // refused with the parser's cause.
    let ssh_fallbacks = match target.get("ssh_fallbacks") {
        None => Vec::new(),
        Some(value) => serde_json::from_value(value.clone()).map_err(|error| {
            ResolveError::Declaration(format!(
                "service {service:?} has invalid SSH fallback paths: {error}"
            ))
        })?,
    };
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
            .checked_add(GENERATION_STEP)
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
    advance_block_generation(document, DIRECTORY_KEY)
}

/// Advance `registry.<key>.generation` by the declared step and return the
/// new value: the directory's counter and release_control's are advanced the
/// same way, so no writer spells its own increment.
pub fn advance_block_generation(document: &mut Value, key: &str) -> Result<u64, String> {
    let block = document
        .get_mut(key)
        .and_then(Value::as_object_mut)
        .ok_or_else(|| format!("registry.{key}: is not an object"))?;
    let current = block
        .get("generation")
        .and_then(Value::as_u64)
        .ok_or_else(|| format!("registry.{key}.generation is not an unsigned integer"))?;
    let next = current
        .checked_add(GENERATION_STEP)
        .ok_or_else(|| format!("registry.{key}.generation overflow"))?;
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

/// [`resolver_config`] for a reader that may meet a target running no
/// resolver: `None` when the registered target declares no
/// `service_resolver`, the error when it is not registered or its
/// declaration does not parse.
pub fn declared_resolver_config(
    document: &Value,
    target: &str,
) -> Result<Option<ResolverConfig>, String> {
    let target_entries = targets(document)?;
    let entry = target_entries
        .get(target)
        .copied()
        .ok_or_else(|| format!("resolver target {target:?} is not registered"))?;
    match entry.get("service_resolver") {
        None => Ok(None),
        Some(value) => serde_json::from_value(value.clone())
            .map(Some)
            .map_err(|error| format!("registry target service_resolver: {error}")),
    }
}
