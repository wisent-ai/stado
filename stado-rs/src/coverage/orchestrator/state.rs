//! The per-universe attempt ledger the orchestrator reads and writes.

use serde_json::{Map, Value};

use crate::config;
use crate::coverage::CoverageError;
use crate::models::json_dumps_pretty_sorted;
use crate::queue::JobStorage;

// ---------------------------------------------------------------------------
// state (<COVERAGE_STATE_PREFIX>/<universe_id>/state.json)
// ---------------------------------------------------------------------------

fn state_path(universe_id: &str) -> String {
    format!("{}/{universe_id}/state.json", config::COVERAGE_STATE_PREFIX)
}

/// Python `state_load`: `{}` when the state blob is absent; corrupt JSON
/// propagates as an error like Python `json.loads`.
pub async fn state_load(store: &JobStorage, universe_id: &str) -> Result<Value, CoverageError> {
    let Some(txt) = store.download_text(&state_path(universe_id)).await? else {
        return Ok(Value::Object(Map::new()));
    };
    Ok(serde_json::from_str(&txt)?)
}

/// Python `state_save`: `json.dumps(state, indent=2, sort_keys=True)`.
pub async fn state_save(
    store: &JobStorage,
    universe_id: &str,
    state: &Value,
) -> Result<(), CoverageError> {
    store
        .upload_text(&state_path(universe_id), &json_dumps_pretty_sorted(state))
        .await?;
    Ok(())
}

/// Mutable access to `state[group_key]`, creating the (object) slot when
/// missing — Python `state.setdefault(group_key, {})`.
pub(in crate::coverage) fn state_slot<'a>(
    state: &'a mut Value,
    group_key: &str,
) -> &'a mut Map<String, Value> {
    if !state.is_object() {
        *state = Value::Object(Map::new());
    }
    let obj = state.as_object_mut().expect("ensured object");
    let slot = obj
        .entry(group_key.to_string())
        .or_insert_with(|| Value::Object(Map::new()));
    if !slot.is_object() {
        *slot = Value::Object(Map::new());
    }
    slot.as_object_mut().expect("ensured object")
}
