//! The reads every source shares: payload fields, edges, states, digests.
//!
//! [`value_text`] and [`object_strings`] pull the strings a provider payload
//! carries under any of a list of keys, [`collect_resource_references`] walks
//! a payload for the identifiers that point at another resource,
//! [`region_from_zone`] narrows a zone to its region, [`source_state`]
//! classifies what a read came back with by its summary, and
//! [`canonical_revision`] and [`sha256_hex`] are the digests a record and a
//! snapshot are addressed by.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;

use crate::autonomy::model::SourceState;
use crate::cli::resources::model::canonical_json_bytes;

pub(super) fn collect_resource_references(value: &Value, output: &mut BTreeSet<String>) {
    match value {
        Value::String(text) => {
            if text.starts_with("/subscriptions/")
                || text.starts_with("https://www.googleapis.com/compute/")
                || text.starts_with("projects/")
                || text.starts_with("i-")
                || text.starts_with("vol-")
                || text.starts_with("ami-")
            {
                output.insert(text.to_string());
                if let Some(tail) = text.rsplit('/').next() {
                    output.insert(tail.to_string());
                }
            }
        }
        Value::Array(items) => {
            for item in items {
                collect_resource_references(item, output);
            }
        }
        Value::Object(object) => {
            for nested in object.values() {
                collect_resource_references(nested, output);
            }
        }
        _ => {}
    }
}

pub(super) fn object_strings(value: Option<&Value>) -> BTreeMap<String, String> {
    value
        .and_then(Value::as_object)
        .into_iter()
        .flatten()
        .filter_map(|(key, value)| value.as_str().map(|text| (key.clone(), text.to_string())))
        .collect()
}

pub(super) fn value_text(value: &Value, keys: &[&str]) -> Option<String> {
    keys.iter()
        .find_map(|key| value.get(*key).and_then(Value::as_str))
        .map(str::to_string)
}

/// A GCP inventory's standing as an autonomy source: complete only when
/// every probe is ok and none reported an error, blocked when a critical
/// probe failed, degraded otherwise.
pub(super) fn source_state(
    summary: crate::providers::gcp::inventory::InventoryState,
    errors: &[String],
) -> SourceState {
    match summary {
        crate::providers::gcp::inventory::InventoryState::Healthy if errors.is_empty() => {
            SourceState::Complete
        }
        crate::providers::gcp::inventory::InventoryState::Critical => SourceState::Blocked,
        crate::providers::gcp::inventory::InventoryState::Healthy
        | crate::providers::gcp::inventory::InventoryState::Degraded => SourceState::Degraded,
    }
}

pub(super) fn region_from_zone(zone: &str) -> Option<&str> {
    zone.rsplit_once('-').map(|(region, _)| region)
}

pub(super) fn canonical_revision(value: &Value) -> Option<String> {
    canonical_json_bytes(value)
        .ok()
        .map(|bytes| sha256_hex(&bytes))
}

pub(super) fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    hex::encode(Sha256::digest(bytes))
}
