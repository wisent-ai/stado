//! Reading single facts back out of the capacity broadcast the host published.

use std::collections::BTreeMap;

use serde_json::Value;

/// One `diag` boolean the agent published, or `None` when this host published
/// nothing or that tick carried no such key.
pub(super) fn diag_flag(payload: Option<&Value>, key: &str) -> Option<bool> {
    payload
        .and_then(|payload| payload.get("diag"))
        .and_then(|diag| diag.get(key))
        .and_then(Value::as_bool)
}

/// Available placements by accelerator class, derived by the worker from live
/// per-card VRAM.
pub(super) fn accelerator_availability(payload: &Value) -> BTreeMap<String, i64> {
    payload
        .get("available_accelerators")
        .and_then(Value::as_object)
        .into_iter()
        .flat_map(|entries| entries.iter())
        .filter_map(|(accelerator, count)| count.as_i64().map(|count| (accelerator.clone(), count)))
        .collect()
}
