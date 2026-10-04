//! Profile selection and the compare-and-swapped transaction claim.

use std::collections::BTreeSet;

use serde_json::Value;

use crate::placement::document::profiles;
use crate::placement::model::{PlacementProfile, PlacementTransaction};
use crate::placement::TRANSACTIONS_KEY;

/// Why no single placement profile answers a move request.
#[derive(Debug)]
pub enum ProfileLookupError {
    /// The request names no service, or one twice.
    Request(String),
    /// No profile holds exactly the requested services.
    NoMatch(String),
    /// The profiles are malformed, or two of them hold the same services.
    Declaration(String),
}

impl std::fmt::Display for ProfileLookupError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Request(detail) | Self::NoMatch(detail) | Self::Declaration(detail) => {
                f.write_str(detail)
            }
        }
    }
}

impl std::error::Error for ProfileLookupError {}

pub fn profile_for_services(
    document: &Value,
    requested: &[String],
) -> Result<PlacementProfile, ProfileLookupError> {
    if requested.is_empty() {
        return Err(ProfileLookupError::Request(
            "placement move requires at least one service".to_string(),
        ));
    }
    let requested_set: BTreeSet<&String> = requested.iter().collect();
    if requested_set.len() != requested.len() {
        return Err(ProfileLookupError::Request(
            "placement move service names must not repeat".to_string(),
        ));
    }
    let matches: Vec<PlacementProfile> = profiles(document)
        .map_err(ProfileLookupError::Declaration)?
        .into_iter()
        .filter(|profile| {
            profile.services.len() == requested.len()
                && profile.services.iter().collect::<BTreeSet<_>>() == requested_set
        })
        .collect();
    match matches.as_slice() {
        [profile] => Ok(profile.clone()),
        [] => Err(ProfileLookupError::NoMatch(format!(
            "no placement profile matches services {}",
            requested.join(" ")
        ))),
        _ => Err(ProfileLookupError::Declaration(format!(
            "services {} match multiple placement profiles",
            requested.join(" ")
        ))),
    }
}

pub fn claim_transaction(
    document: &mut Value,
    transaction: &PlacementTransaction,
) -> Result<(), String> {
    let root = document
        .as_object_mut()
        .ok_or_else(|| "registry: must be an object".to_string())?;
    let active = root
        .entry(TRANSACTIONS_KEY.to_string())
        .or_insert_with(|| Value::Array(Vec::new()))
        .as_array_mut()
        .ok_or_else(|| format!("registry.{TRANSACTIONS_KEY}: must be an array"))?;
    if !active.is_empty() {
        return Err(format!(
            "another placement transaction is active: {}",
            active
                .iter()
                .filter_map(|value| value.get("id").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    active.push(
        serde_json::to_value(transaction)
            .map_err(|error| format!("could not serialize placement transaction: {error}"))?,
    );
    Ok(())
}

pub fn release_transaction(document: &mut Value, id: &str) -> Result<bool, String> {
    let root = document
        .as_object_mut()
        .ok_or_else(|| "registry: must be an object".to_string())?;
    let Some(active) = root.get_mut(TRANSACTIONS_KEY) else {
        return Ok(false);
    };
    let active = active
        .as_array_mut()
        .ok_or_else(|| format!("registry.{TRANSACTIONS_KEY}: must be an array"))?;
    let previous_len = active.len();
    active.retain(|value| value.get("id").and_then(Value::as_str) != Some(id));
    let removed = active.len() != previous_len;
    if active.is_empty() {
        root.remove(TRANSACTIONS_KEY);
    }
    Ok(removed)
}
