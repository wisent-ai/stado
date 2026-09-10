use crate::targets::*;

/// Validate a registry-v2 document without modifying it. Python returns the
/// input dict; here the borrowed input simply remains valid on `Ok(())`.
pub fn validate_registry(data: &Value) -> Result<(), RegistryValidationError> {
    validate_registry_body(data, true)
}

/// The whole check. `include_inference` is false only for
/// [`validate_registry_for_write`], which re-runs that section itself so it can
/// scope a failure to writes that actually touch it.
pub(crate) fn validate_registry_body(
    data: &Value,
    include_inference: bool,
) -> Result<(), RegistryValidationError> {
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

        if let Some(reclaim) = target.get("memory_reclaim") {
            if !crate::capabilities::ProviderId::Local.matches(kind) {
                return Err(verr(
                    &format!("{location}.memory_reclaim"),
                    "is allowed only for kind='local'",
                ));
            }
            let reclaim_location = format!("{location}.memory_reclaim");
            crate::providers::local::host_memory::validate::validate(reclaim, &reclaim_location)
                .map_err(|problem| verr(&problem.location, &problem.message))?;
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
    // The unit-image revisit policy is a top-level, unmodelled key, so it
    // round-trips through `Registry::extra` and older builds preserve it
    // without reading it. Validating it here is what makes an operator learn
    // at the write, and what makes a build that disagrees with the document
    // report it through the existing `build-refuses-registry` finding rather
    // than act on the part it understood.
    crate::release_unit_image::validate_registry_contract(data).map_err(RegistryValidationError)?;
    // The public-origin block is judged here for the same reason: an origin
    // nothing outside the tailnet can resolve reached the public release
    // route through an untyped deployment variable, and no reader refused it.
    crate::public_origin::validate_registry_contract(data).map_err(RegistryValidationError)?;

    if include_inference {
        crate::inference::schema::validate(data).map_err(RegistryValidationError)?;
    }
    Ok(())
}
