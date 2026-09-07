use super::*;

pub(crate) fn validate_service_onboarding(
    target: &Map<String, Value>,
    location: &str,
) -> Result<(), RegistryValidationError> {
    let Some(services) = target.get("services") else {
        return Ok(());
    };
    let services = services
        .as_array()
        .ok_or_else(|| verr(&format!("{location}.services"), "must be an array"))?;
    for (index, service) in services.iter().enumerate() {
        let service_location = format!("{location}.services[{index}]");
        let service = service
            .as_object()
            .ok_or_else(|| verr(&service_location, "must be an object"))?;
        let Some(onboarding) = service.get("onboarding") else {
            continue;
        };
        let onboarding_location = format!("{service_location}.onboarding");
        let onboarding = onboarding
            .as_object()
            .ok_or_else(|| verr(&onboarding_location, "must be an object"))?;
        const KEYS: [&str; 7] = [
            "display_name",
            "first_success_fact",
            "onboarding_kind",
            "product_id",
            "repository",
            "status",
            "surface_kinds",
        ];
        let keys: HashSet<&str> = onboarding.keys().map(String::as_str).collect();
        if keys != KEYS.into_iter().collect() {
            return Err(verr(
                &onboarding_location,
                &format!("must contain exactly {}", py_list_repr(&KEYS)),
            ));
        }
        for field in ["product_id", "first_success_fact"] {
            if !onboarding[field]
                .as_str()
                .is_some_and(is_product_identifier)
            {
                return Err(verr(
                    &format!("{onboarding_location}.{field}"),
                    "must be a product identifier",
                ));
            }
        }
        if !onboarding["display_name"]
            .as_str()
            .is_some_and(|value| !value.trim().is_empty() && value.len() <= 512)
        {
            return Err(verr(
                &format!("{onboarding_location}.display_name"),
                "must be a non-empty string of at most 512 bytes",
            ));
        }
        if !onboarding["repository"].as_str().is_some_and(is_repository) {
            return Err(verr(
                &format!("{onboarding_location}.repository"),
                "must be an owner/repository identifier",
            ));
        }
        let surfaces = onboarding["surface_kinds"].as_array().ok_or_else(|| {
            verr(
                &format!("{onboarding_location}.surface_kinds"),
                "must be an array",
            )
        })?;
        const SURFACES: [&str; 10] = [
            "web", "ios", "android", "macos", "desktop", "cli", "api", "worker", "operator",
            "python",
        ];
        let mut seen = HashSet::new();
        if surfaces.is_empty()
            || surfaces.iter().any(|surface| {
                !surface
                    .as_str()
                    .is_some_and(|value| SURFACES.contains(&value) && seen.insert(value))
            })
        {
            return Err(verr(
                &format!("{onboarding_location}.surface_kinds"),
                "must contain unique supported surfaces",
            ));
        }
        if !matches!(
            onboarding["onboarding_kind"].as_str(),
            Some("human" | "machine" | "both")
        ) {
            return Err(verr(
                &format!("{onboarding_location}.onboarding_kind"),
                "must be human, machine, or both",
            ));
        }
        if !matches!(
            onboarding["status"].as_str(),
            Some("planned" | "active" | "archived")
        ) {
            return Err(verr(
                &format!("{onboarding_location}.status"),
                "must be planned, active, or archived",
            ));
        }
    }
    Ok(())
}
