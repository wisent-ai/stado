//! One route at a time: which host a profile makes active, whether a
//! release-controlled product may serve it, and what an endpoint and a
//! resolver configuration have to look like.

use std::collections::{BTreeMap, BTreeSet};
use std::net::{IpAddr, SocketAddr};

use serde_json::Value;

use super::super::{ResolverConfig, ServiceDirectory, ServiceEndpoint, ServiceRoute};
use super::super::self_reference::socket_of;
use super::{target_declares_service, validate_identifier};

pub(super) fn active_profile_host(
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
pub(super) fn release_controlled_product(
    profile: &crate::placement::PlacementProfile,
    service: &str,
) -> Result<Option<String>, String> {
    let mut product = None;
    let mut external_hosts = Vec::new();
    for (host, host_profile) in &profile.hosts {
        let unit = host_profile.units.get(service).ok_or_else(|| {
            format!(
                "placement profile {:?} has no unit for service {service:?} on {host:?}",
                profile.name
            )
        })?;
        if let Some(owner) = unit.release_controlled() {
            if unit.name != service {
                return Err(format!(
                    "placement profile {:?} release-controlled service {service:?} must retain \
                     that exact logical name, not {:?}",
                    profile.name, unit.name
                ));
            }
            external_hosts.push(host.as_str());
            match product.as_deref() {
                None => product = Some(owner.product.clone()),
                Some(existing) if existing == owner.product => {}
                Some(existing) => {
                    return Err(format!(
                        "placement profile {:?} service {service:?} mixes release products \
                         {existing:?} and {:?}",
                        profile.name, owner.product
                    ))
                }
            }
        }
    }
    if external_hosts.is_empty() {
        return Ok(None);
    }
    if external_hosts.len() != profile.hosts.len() {
        return Err(format!(
            "placement profile {:?} service {service:?} must be release-controlled in every host \
             template or managed in every host template",
            profile.name
        ));
    }
    Ok(product)
}

pub(super) fn validate_release_controlled_route(
    document: &Value,
    target_entries: &BTreeMap<String, &Value>,
    profile: &crate::placement::PlacementProfile,
    service: &str,
    route: &ServiceRoute,
    product: &str,
    location: &str,
) -> Result<(), String> {
    for (host, target) in target_entries {
        if target_declares_service(target, service) {
            return Err(format!(
                "{location}: release-controlled service {service:?} must not retain a \
                 targets[].services[] lifecycle record on {host:?}"
            ));
        }
    }
    let control = crate::release_control::control(document)?.ok_or_else(|| {
        format!("{location}: release-controlled service requires release_control")
    })?;
    let policy = control.products.get(product).ok_or_else(|| {
        format!("{location}: release-control product {product:?} is not declared")
    })?;
    if policy.service != service {
        return Err(format!(
            "{location}: release-control product {product:?} owns logical service {:?}, not \
             {service:?}",
            policy.service
        ));
    }
    let raw_targets = document
        .get("release_control")
        .and_then(|control| control.get("products"))
        .and_then(|products| products.get(product))
        .and_then(|policy| policy.get("targets"))
        .and_then(Value::as_object)
        .ok_or_else(|| format!("{location}: release-control product targets are not an object"))?;
    for (target, release_target) in raw_targets {
        if release_target.get("legacy_launchd_label").is_some()
            || release_target.get("legacy_launchd_plist").is_some()
        {
            return Err(format!(
                "{location}: release-controlled service {service:?} must not retain legacy \
                 launchd restore fields on release target {target:?}"
            ));
        }
    }
    let target_policy = policy.targets.get(&route.active_host).ok_or_else(|| {
        format!(
            "{location}: release-control product {product:?} has no target {:?}",
            route.active_host
        )
    })?;
    let serving = target_policy.blue_green_serving().map_err(|error| {
        format!("{location}: release-controlled placement requires a blue-green target: {error}")
    })?;
    let stable_bind = serving
        .stable_bind
        .parse::<SocketAddr>()
        .map_err(|error| format!("{location}: invalid release stable_bind: {error}"))?;
    let endpoint = route
        .endpoints
        .get(&route.active_host)
        .and_then(|endpoint| socket_of(&endpoint.url))
        .ok_or_else(|| {
            format!(
                "{location}: active release-controlled host {:?} has no usable endpoint",
                route.active_host
            )
        })?;
    if endpoint != stable_bind {
        return Err(format!(
            "{location}: active endpoint {endpoint} must equal release-control stable_bind \
             {stable_bind}"
        ));
    }
    let probe = profile
        .hosts
        .get(&route.active_host)
        .and_then(|host| host.probes.iter().find(|probe| probe.service == service))
        .and_then(|probe| socket_of(&probe.url))
        .ok_or_else(|| {
            format!(
                "{location}: active release-controlled host {:?} has no usable placement probe",
                route.active_host
            )
        })?;
    if probe != stable_bind {
        return Err(format!(
            "{location}: placement probe socket {probe} must equal release-control stable_bind \
             {stable_bind}"
        ));
    }
    Ok(())
}

pub(super) fn validate_endpoint(endpoint: &ServiceEndpoint, location: &str) -> Result<(), String> {
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
    if let Some(base_path) = &endpoint.base_path {
        // An absolute, single-segment-or-deeper path with no trailing slash,
        // so composing it onto the origin is textual and unambiguous. A
        // consumer must never have to decide whether to strip a slash.
        if !base_path.starts_with('/')
            || base_path.ends_with('/')
            || base_path.contains("//")
            || base_path.contains('?')
            || base_path.contains('#')
            || base_path.len() < 2
        {
            return Err(format!(
                "{location}.base_path: must be an absolute path with no trailing slash"
            ));
        }
    }
    Ok(())
}

pub(super) fn validate_resolver_config(
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
        if adapter.connect_seconds == 0 {
            return Err(format!(
                "{adapter_location}.connect_seconds: must be positive"
            ));
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
