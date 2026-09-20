//! Per-boundary authorization policy, credentials and endpoints.

mod data;
mod delivery;
mod object;
mod secrets;

/// The actions each authenticated API admits, from `api-actions.json`
/// beside this module: the words a Skarbiec grant is written in. Each
/// boundary reads its own key; none of them spells the words again.
pub(crate) fn declared_actions(api: &str) -> Vec<String> {
    let document: serde_json::Value = serde_json::from_str(include_str!("api-actions.json"))
        .expect("api-actions.json beside this module is valid JSON");
    document[api]
        .as_array()
        .unwrap_or_else(|| panic!("api-actions.json declares no {api} actions"))
        .iter()
        .filter_map(|action| action.as_str().map(str::to_owned))
        .collect()
}

pub use data::*;
pub use delivery::*;
pub use object::*;
pub use secrets::*;
