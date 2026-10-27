//! Everything a registry has to satisfy before its directory is trusted.
//!
//! `contract` walks the whole document; `routes` checks one route, endpoint
//! and resolver at a time. What both need — the identifier shape and the
//! target index — is here.

use std::collections::BTreeMap;

use serde_json::Value;

mod contract;
mod ports;
mod routes;

pub use contract::validate_registry_contract;

pub(super) fn identifier(value: &str) -> bool {
    crate::primitives::word::lowercase_identifier(value)
}

pub(super) fn validate_identifier(value: &str, location: &str) -> Result<(), String> {
    if identifier(value) {
        Ok(())
    } else {
        Err(format!(
            "{location}: must be a lowercase identifier without empty edges"
        ))
    }
}

pub(super) fn targets(document: &Value) -> Result<BTreeMap<String, &Value>, String> {
    let entries = document
        .get("targets")
        .and_then(Value::as_array)
        .ok_or_else(|| "registry.targets: must be an array".to_string())?;
    Ok(entries
        .iter()
        .filter_map(|entry| {
            entry
                .get("name")
                .and_then(Value::as_str)
                .map(|name| (name.to_string(), entry))
        })
        .collect())
}

/// Whether `target` declares `service` under its own name or the unit the
/// service catalog gives it. A catalog that cannot be read is the error: a
/// service declared under its unit would be read as undeclared.
pub(super) fn target_declares_service(target: &Value, service: &str) -> Result<bool, String> {
    let catalog_unit = match crate::deploy::service_catalog::lookup(service) {
        Ok(entry) => entry.and_then(|entry| entry.unit),
        Err(cause) => {
            return Err(format!(
                "the service catalog could not be read for {service:?}, so whether a target \
                 declares it is unknown: {cause}"
            ))
        }
    };
    Ok(target
        .get("services")
        .and_then(Value::as_array)
        .is_some_and(|services| {
            services.iter().any(|entry| {
                crate::deploy::service::NAME_KEYS.iter().any(|field| {
                    let value = entry.get(field).and_then(Value::as_str);
                    value == Some(service)
                        || catalog_unit
                            .as_deref()
                            .is_some_and(|unit| value == Some(unit))
                })
            })
        }))
}
