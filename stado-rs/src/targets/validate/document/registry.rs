use crate::targets::*;

use super::coordinators::{validate_coordinators, validate_product_contracts};

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
    let required_version = registry_schema_version();
    let version_ok = root
        .get("schema_version")
        .is_some_and(|v| !v.is_boolean() && v.as_i64() == Some(required_version));
    if !version_ok {
        return Err(verr(
            "registry.schema_version",
            &format!("must be {required_version}"),
        ));
    }

    let targets = root
        .get("targets")
        .and_then(Value::as_array)
        .ok_or_else(|| verr("registry.targets", "must be an array"))?;

    let mut names: HashSet<&str> = HashSet::new();
    let mut identities: HashMap<String, (usize, String)> = HashMap::new();
    let mut target_heuristics: HashMap<&str, &str> = HashMap::new();
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

        // The kind is one of the declared provider kinds; a target that
        // declares none, or declares it in another shape, is refused the
        // same way.
        let kind = match target.get("kind").and_then(Value::as_str) {
            Some(kind) if valid_kinds.contains(&kind) => kind,
            Some(_) | None => {
                return Err(verr(
                    &format!("{location}.kind"),
                    &format!("must be one of {}", py_list_repr(&valid_kinds)),
                ))
            }
        };
        if let Some(value) = target.get("gpu_power_limit_watts") {
            let watts = value
                .as_u64()
                .and_then(std::num::NonZeroU64::new)
                .map(std::num::NonZeroU64::get)
                .ok_or_else(|| {
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
        if let Some(tariff) = target.get("power_tariff") {
            let at = format!("{location}.power_tariff");
            let object = tariff
                .as_object()
                .ok_or_else(|| verr(&at, "must be {price_per_kwh, currency, source}"))?;
            let price = object.get("price_per_kwh").and_then(Value::as_f64);
            if !price.is_some_and(|price| price.is_finite() && price >= 0.0) {
                return Err(verr(
                    &format!("{at}.price_per_kwh"),
                    "must be a number at or above zero",
                ));
            }
            for field in ["currency", "source"] {
                if !object
                    .get(field)
                    .and_then(Value::as_str)
                    .is_some_and(|text| !text.trim().is_empty())
                {
                    return Err(verr(
                        &format!("{at}.{field}"),
                        if field == "source" {
                            "must name where the price was read (the supplier's bill or \
                             published tariff), so no host-hour is priced at a number nobody \
                             stated"
                        } else {
                            "must be a non-empty currency code"
                        },
                    ));
                }
            }
            if let Some(rate) = object.get("usd_rate") {
                let rate_at = format!("{at}.usd_rate");
                let rate = rate
                    .as_object()
                    .ok_or_else(|| verr(&rate_at, "must be {usd_per_unit, as_of, source}"))?;
                if !rate
                    .get("usd_per_unit")
                    .and_then(Value::as_f64)
                    .is_some_and(|value| value.is_finite() && value > 0.0)
                {
                    return Err(verr(
                        &format!("{rate_at}.usd_per_unit"),
                        "must be a number above zero: the US dollars one unit of the tariff's \
                         currency was worth",
                    ));
                }
                for field in ["as_of", "source"] {
                    if !rate
                        .get(field)
                        .and_then(Value::as_str)
                        .is_some_and(|text| !text.trim().is_empty())
                    {
                        return Err(verr(
                            &format!("{rate_at}.{field}"),
                            "must name the date of the rate and the published table it was \
                             read from, so no hour is converted at a rate nobody stated",
                        ));
                    }
                }
                if let Some(unknown) = rate
                    .keys()
                    .find(|key| !["usd_per_unit", "as_of", "source"].contains(&key.as_str()))
                {
                    return Err(verr(&format!("{rate_at}.{unknown}"), "is not a rate field"));
                }
            }
            if let Some(unknown) = object.keys().find(|key| {
                !["price_per_kwh", "currency", "source", "usd_rate"].contains(&key.as_str())
            }) {
                return Err(verr(&format!("{at}.{unknown}"), "is not a tariff field"));
            }
        }
        if let Some(hardware) = target.get("hardware") {
            let at = format!("{location}.hardware");
            let object = hardware.as_object().ok_or_else(|| {
                verr(
                    &at,
                    "must be {purchase_price, currency, service_years, source}",
                )
            })?;
            for field in ["purchase_price", "service_years"] {
                if !object
                    .get(field)
                    .and_then(Value::as_f64)
                    .is_some_and(|value| value.is_finite() && value > 0.0)
                {
                    return Err(verr(
                        &format!("{at}.{field}"),
                        "must be a number above zero",
                    ));
                }
            }
            for field in ["currency", "source"] {
                if !object
                    .get(field)
                    .and_then(Value::as_str)
                    .is_some_and(|text| !text.trim().is_empty())
                {
                    return Err(verr(
                        &format!("{at}.{field}"),
                        if field == "source" {
                            "must name where the price was read (the invoice), so no hour \
                             carries wear nobody stated"
                        } else {
                            "must be a non-empty currency code"
                        },
                    ));
                }
            }
            if let Some(unknown) = object.keys().find(|key| {
                !["purchase_price", "currency", "service_years", "source"].contains(&key.as_str())
            }) {
                return Err(verr(&format!("{at}.{unknown}"), "is not a hardware field"));
            }
        }
        let platform_location = format!("{location}.release_platform");
        // The platform is one of the shipped names; a target that declares
        // none, or declares it in another shape, is refused the same way.
        let platform = match target.get("release_platform").and_then(Value::as_str) {
            Some(platform) if crate::deploy::products::PLATFORMS.contains(&platform) => platform,
            Some(_) | None => {
                return Err(verr(
                    &platform_location,
                    &format!(
                        "must be one of {} and must be confirmed by host inventory",
                        py_list_repr(crate::deploy::products::PLATFORMS)
                    ),
                ))
            }
        };
        validate_privacy_grants(&location, target, platform)?;
        validate_console(&location, target, platform)?;
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
                super::services::validate_service_record(&service_location, service)?;
            }
        }

        if let Some(weles) = target.get("weles") {
            let weles_location = format!("{location}.weles");
            if !crate::capabilities::ProviderId::Local.matches(kind) {
                return Err(verr(&weles_location, "is allowed only for kind='local'"));
            }
            // The block's shape is the policy the registry loads: one
            // deserialization refuses an unknown, missing or mistyped field.
            let policy = <crate::targets::WelesPolicy as serde::Deserialize>::deserialize(weles)
                .map_err(|error| verr(&weles_location, &error.to_string()))?;
            validate_action_list(&weles["actions"], &format!("{weles_location}.actions"))?;
            if let Some(recordings_dir) = &policy.recordings_dir {
                if !recordings_dir.starts_with('/') {
                    return Err(verr(
                        &format!("{weles_location}.recordings_dir"),
                        "must be an absolute path string",
                    ));
                }
            }
        }
        validate_service_onboarding(target, &location)?;

        if let Some(work_root) = target.get("work_root") {
            if !crate::capabilities::ProviderId::Local.matches(kind) {
                return Err(verr(
                    &format!("{location}.work_root"),
                    "is allowed only for kind='local'",
                ));
            }
            if let Err(problem) = work_root
                .as_str()
                .ok_or_else(|| "must be a string".to_string())
                .and_then(crate::providers::local::work_base::validate_declared)
            {
                return Err(verr(&format!("{location}.work_root"), &problem));
            }
        }

        // An identity belongs to one host, and two connection paths still may
        // not name one identity. A connection path that dials the host by one
        // of its own declared hostnames is the same host, not a second one:
        // that name (its .local name) is what follows a lease-assigned address.
        let is_hostname = |location: &str| location.contains(".hostnames[");
        let is_route = |location: &str| location.contains(".ssh");
        for (identity, identity_location) in target_identities(target, &location)? {
            // The route takes the hostname's place, so a second route naming
            // the same identity is refused against the first route.
            let own_name = identities.get(&identity).is_some_and(|(owner, previous)| {
                *owner == index && is_hostname(previous) && is_route(&identity_location)
            });
            if let Some((_, previous)) = identities.get(&identity).filter(|_| !own_name) {
                return Err(verr(
                    &identity_location,
                    &format!("host identity '{identity}' is already declared by {previous}"),
                ));
            }
            identities.insert(identity, (index, identity_location));
        }
    }
    crate::release_pipeline::destinations::validate(data).map_err(RegistryValidationError)?;
    validate_coordinators(root, &target_heuristics)?;
    validate_product_contracts(data, include_inference)
}
