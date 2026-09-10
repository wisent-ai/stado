//! The whole-document check: every service, every route, every consumer and
//! every host resolver, read together, because most of what can be wrong here
//! is only wrong in combination.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path};

use serde_json::Value;

use super::super::{directory, DIRECTORY_KEY};
use super::routes::{
    active_profile_host, release_controlled_product, validate_endpoint,
    validate_release_controlled_route, validate_resolver_config,
};
use super::{target_declares_service, targets, validate_identifier};

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
    let authority = target_entries[&directory.authority.target];
    let has_primary = authority
        .get("ssh")
        .and_then(Value::as_str)
        .is_some_and(|destination| !destination.trim().is_empty());
    let has_fallback = authority
        .get("ssh_fallbacks")
        .and_then(Value::as_array)
        .is_some_and(|paths| !paths.is_empty());
    if !has_primary && !has_fallback {
        return Err(format!(
            "registry.{DIRECTORY_KEY}.authority.target: must declare an SSH connection path"
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
        if let Some(declaration) = route.declaration.as_ref() {
            let problems = crate::declaration::validate(&location, declaration);
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
            if let Some(product) = release_controlled_product(profile, name)? {
                validate_release_controlled_route(
                    document,
                    &target_entries,
                    profile,
                    name,
                    route,
                    &product,
                    &location,
                )?;
            } else {
                let declared_host = active_profile_host(profile, name, &target_entries)?;
                if route.active_host != declared_host {
                    return Err(format!(
                        "{location}.active_host: must match the managed unit on {declared_host:?}"
                    ));
                }
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
