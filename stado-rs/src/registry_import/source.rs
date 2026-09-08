//! Validation of a complete registry document, and decoding of the source
//! bytes into one, before the canonical destination is ever opened.

use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::HashSet;

use crate::targets;

use super::receipt::RegistryImportReceipt;

pub(super) fn validate_document(document: &Value) -> Result<(), String> {
    targets::validate_registry(document).map_err(|error| error.to_string())?;
    crate::cli::fleet::fleets::parse_fleets(document)?;
    for section in ["targets", "fleets", "coordinators", "placement_profiles"] {
        named_entries(document.get(section), section)?;
    }
    Ok(())
}

pub(super) fn named_entries<'a>(
    value: Option<&'a Value>,
    section: &str,
) -> Result<Vec<(&'a str, &'a Value)>, String> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    let entries = value
        .as_array()
        .ok_or_else(|| format!("registry.{section}: must be an array"))?;
    let mut names = HashSet::with_capacity(entries.len());
    let mut named = Vec::with_capacity(entries.len());
    for (index, entry) in entries.iter().enumerate() {
        let name = entry
            .get("name")
            .and_then(Value::as_str)
            .filter(|name| !name.is_empty())
            .ok_or_else(|| {
                format!("registry.{section}[{index}].name: must be a non-empty string")
            })?;
        if !names.insert(name) {
            return Err(format!(
                "registry.{section}[{index}].name: duplicate name {name:?}"
            ));
        }
        named.push((name, entry));
    }
    Ok(named)
}

pub(super) fn source_rejection(bytes: &[u8], reason: String) -> RegistryImportReceipt {
    let mut receipt =
        RegistryImportReceipt::empty(format!("{:x}", Sha256::digest(bytes)), "rejected");
    receipt.rejected.push(reason);
    receipt
}

/// Decode and validate the complete source without reading or mutating the
/// canonical destination. Rejections therefore cannot leave a partial import.
pub(super) fn decode_source(bytes: &[u8]) -> Result<Value, String> {
    let document: Value = serde_json::from_slice(bytes)
        .map_err(|error| format!("source is not valid JSON: {error}"))?;
    validate_document(&document).map_err(|error| format!("source registry is invalid: {error}"))?;
    Ok(document)
}
