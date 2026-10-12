use crate::targets::*;

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
        // The block's shape, field names and vocabularies are the record's
        // own: one deserialization refuses an unknown or missing field and a
        // surface, kind or status outside its enum, naming it.
        let onboarding =
            <crate::deploy::service::OnboardingProduct as serde::Deserialize>::deserialize(
                onboarding,
            )
            .map_err(|error| verr(&onboarding_location, &error.to_string()))?;
        for (field, value) in [
            ("product_id", &onboarding.product_id),
            ("first_success_fact", &onboarding.first_success_fact),
        ] {
            if !is_product_identifier(value) {
                return Err(verr(
                    &format!("{onboarding_location}.{field}"),
                    "must be a product identifier",
                ));
            }
        }
        if onboarding.display_name.trim().is_empty() {
            return Err(verr(
                &format!("{onboarding_location}.display_name"),
                "must be a non-empty string",
            ));
        }
        if !is_repository(&onboarding.repository) {
            return Err(verr(
                &format!("{onboarding_location}.repository"),
                "must be an owner/repository identifier",
            ));
        }
        let mut seen = HashSet::new();
        if onboarding.surface_kinds.is_empty()
            || !onboarding
                .surface_kinds
                .iter()
                .all(|kind| seen.insert(*kind))
        {
            return Err(verr(
                &format!("{onboarding_location}.surface_kinds"),
                "must contain unique supported surfaces",
            ));
        }
    }
    Ok(())
}
