//! Everything a registry has to satisfy before its directory is trusted.
//!
//! `contract` walks the whole document; `routes` checks one route, endpoint
//! and resolver at a time. What both need — the identifier shape and the
//! target index — is here.

use std::collections::BTreeMap;

use serde_json::Value;

mod contract;
mod routes;

pub use contract::validate_registry_contract;

pub(super) fn identifier(value: &str) -> bool {
    let bytes = value.as_bytes();
    let edge = |byte: u8| byte.is_ascii_lowercase() || byte.is_ascii_digit();
    !bytes.is_empty()
        && edge(bytes[0])
        && edge(bytes[bytes.len() - 1])
        && bytes
            .iter()
            .all(|byte| edge(*byte) || matches!(byte, b'.' | b'_' | b'-'))
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

pub(super) fn target_declares_service(target: &Value, service: &str) -> bool {
    let catalog_unit = crate::deploy::service_catalog::lookup(service)
        .ok()
        .flatten()
        .and_then(|entry| entry.unit);
    target
        .get("services")
        .and_then(Value::as_array)
        .is_some_and(|services| {
            services.iter().any(|entry| {
                ["name", "label", "unit"].iter().any(|field| {
                    let value = entry.get(field).and_then(Value::as_str);
                    value == Some(service)
                        || catalog_unit
                            .as_deref()
                            .is_some_and(|unit| value == Some(unit))
                })
            })
        })
}
