use crate::deploy::service::*;

// ---------------------------------------------------------------------------
// Registry document mutation (pure; the write goes through cli/registry.rs)
// ---------------------------------------------------------------------------

/// Borrow one kind=local target object out of the raw canonical document.
fn target_entry<'a>(
    document: &'a mut Value,
    host: &str,
) -> Result<&'a mut Map<String, Value>, DeployError> {
    let targets = document
        .get_mut("targets")
        .and_then(Value::as_array_mut)
        .ok_or_else(|| DeployError("registry.targets: must be an array".to_string()))?;
    let entry = targets
        .iter_mut()
        .find(|entry| entry.get("name").and_then(Value::as_str) == Some(host))
        .ok_or_else(|| {
            DeployError(format!(
                "target {} is not in the canonical registry",
                py_str_repr(host)
            ))
        })?;
    let entry = entry
        .as_object_mut()
        .ok_or_else(|| DeployError("registry target must be an object".to_string()))?;
    if entry.get("kind").and_then(Value::as_str) != Some("local") {
        return Err(DeployError(format!(
            "target {} is not a local host",
            py_str_repr(host)
        )));
    }
    Ok(entry)
}

/// Declare a service in the canonical document.
///
/// Pure by design: the caller reads the document with its generation through
/// `cli/registry.rs::{commit_document, fetch_versioned_document}`, applies
/// this, and writes it back conditionally on that generation, which validates
/// the whole document before it writes anything.
pub fn add_service(document: &mut Value, service: &ManagedService) -> Result<(), DeployError> {
    let entry = target_entry(document, &service.host)?;
    let declared = entry
        .entry(SERVICES_KEY)
        .or_insert_with(|| Value::Array(Vec::new()))
        .as_array_mut()
        .ok_or_else(|| {
            DeployError(format!(
                "registry target {} has a non-array {SERVICES_KEY} key",
                py_str_repr(&service.host)
            ))
        })?;
    let taken = declared
        .iter()
        .filter_map(Value::as_object)
        .map(|record| ManagedService::from_record(&service.host, record))
        .any(|existing| existing.matches(service.unit_id()) || existing.matches(&service.name));
    if taken {
        return Err(DeployError(format!(
            "the registry already manages {} on {}",
            py_str_repr(service.unit_id()),
            py_str_repr(&service.host)
        )));
    }
    declared.push(service.to_record());
    Ok(())
}

/// Replace one registry-managed service after a host observation corrected its
/// unit identity or file path. The match is by logical name or stable unit id;
/// recovery-sourced services are never written into the registry.
pub fn replace_service(document: &mut Value, service: &ManagedService) -> Result<(), DeployError> {
    let entry = target_entry(document, &service.host)?;
    let declared = entry
        .get_mut(SERVICES_KEY)
        .and_then(Value::as_array_mut)
        .ok_or_else(|| {
            DeployError(format!(
                "{} declares no managed services",
                py_str_repr(&service.host)
            ))
        })?;
    let record = declared
        .iter_mut()
        .find(|record| {
            record.as_object().is_some_and(|record| {
                let existing = ManagedService::from_record(&service.host, record);
                existing.matches(&service.name) || existing.matches(service.unit_id())
            })
        })
        .ok_or_else(|| {
            DeployError(format!(
                "{} is not a registry-managed service on {}",
                py_str_repr(&service.name),
                py_str_repr(&service.host)
            ))
        })?;
    *record = service.to_record();
    Ok(())
}

/// Attach product onboarding metadata to one already managed service.
pub fn set_service_onboarding(
    document: &mut Value,
    host: &str,
    service: &str,
    onboarding: OnboardingProduct,
) -> Result<ManagedService, DeployError> {
    let entry = target_entry(document, host)?;
    let declared = entry
        .get_mut(SERVICES_KEY)
        .and_then(Value::as_array_mut)
        .ok_or_else(|| {
            DeployError(format!(
                "{} declares no managed services",
                py_str_repr(host)
            ))
        })?;
    let record = declared
        .iter_mut()
        .find(|record| {
            record
                .as_object()
                .is_some_and(|record| ManagedService::from_record(host, record).matches(service))
        })
        .and_then(Value::as_object_mut)
        .ok_or_else(|| {
            DeployError(format!(
                "{} is not a registry-managed service on {}",
                py_str_repr(service),
                py_str_repr(host)
            ))
        })?;
    record.insert(
        "onboarding".to_string(),
        serde_json::to_value(&onboarding)
            .map_err(|error| DeployError(format!("invalid onboarding product: {error}")))?,
    );
    Ok(ManagedService::from_record(host, record))
}

/// Undeclare a service. Removing the last one drops the key entirely, so a
/// host with nothing declared reads the same as one that never declared
/// anything.
pub fn remove_service(
    document: &mut Value,
    host: &str,
    unit: &str,
) -> Result<ManagedService, DeployError> {
    let entry = target_entry(document, host)?;
    let declared = entry
        .get_mut(SERVICES_KEY)
        .and_then(Value::as_array_mut)
        .ok_or_else(|| {
            DeployError(format!(
                "{} declares no managed services",
                py_str_repr(host)
            ))
        })?;
    // Position over the array itself, not over a filtered view: a record
    // that is not an object still occupies a slot, and an index taken from
    // a filtered sequence would delete the wrong one.
    let found = declared.iter().position(|record| {
        record
            .as_object()
            .is_some_and(|record| ManagedService::from_record(host, record).matches(unit))
    });
    let Some(index) = found else {
        return Err(DeployError(format!(
            "{} is not a registry-managed service on {}",
            py_str_repr(unit),
            py_str_repr(host)
        )));
    };
    let removed = declared.remove(index);
    let now_empty = declared.is_empty();
    let removed = removed
        .as_object()
        .map(|record| ManagedService::from_record(host, record))
        .unwrap_or_default();
    if now_empty {
        entry.remove(SERVICES_KEY);
    }
    Ok(removed)
}
