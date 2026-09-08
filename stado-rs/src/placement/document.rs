//! Typed reads of the placement sections of a registry document.

use serde_json::{Map, Value};

use super::model::{PlacementProfile, PlacementTransaction};
use super::{PROFILES_KEY, TRANSACTIONS_KEY};

pub fn profiles(document: &Value) -> Result<Vec<PlacementProfile>, String> {
    match document.get(PROFILES_KEY) {
        None => Ok(Vec::new()),
        Some(value) => serde_json::from_value(value.clone())
            .map_err(|error| format!("registry.{PROFILES_KEY}: {error}")),
    }
}

pub fn transactions(document: &Value) -> Result<Vec<PlacementTransaction>, String> {
    match document.get(TRANSACTIONS_KEY) {
        None => Ok(Vec::new()),
        Some(value) => serde_json::from_value(value.clone())
            .map_err(|error| format!("registry.{TRANSACTIONS_KEY}: {error}")),
    }
}

pub fn root_object(document: &mut Value) -> Result<&mut Map<String, Value>, String> {
    document
        .as_object_mut()
        .ok_or_else(|| "registry: must be an object".to_string())
}
