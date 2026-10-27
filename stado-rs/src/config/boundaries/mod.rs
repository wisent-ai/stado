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
    // The document is compiled in, so a key it lacks or an entry that is
    // not a word is a defect of this build, not of any host's configuration.
    let actions = match document.get(api).and_then(serde_json::Value::as_array) {
        Some(actions) => actions,
        None => panic!("api-actions.json declares no {api} actions"),
    };
    actions
        .iter()
        .map(|action| match action.as_str() {
            Some(word) => word.to_owned(),
            None => panic!("api-actions.json names a {api} action that is not a word: {action}"),
        })
        .collect()
}

/// Check that `value` carries only the fields the struct `Fields` declares.
/// `Fields` is a `#[serde(deny_unknown_fields)]` struct of
/// `Option<IgnoredAny>` members: serde refuses an unknown key with its name
/// and the expected list, recorded under `location`. Values are judged by
/// the caller. Returns whether the field set was accepted.
pub(crate) fn declared_fields<Fields: serde::de::DeserializeOwned>(
    value: &serde_json::Value,
    location: &str,
    problems: &mut Vec<String>,
) -> bool {
    match <Fields as serde::Deserialize>::deserialize(value) {
        Ok(_) => true,
        Err(error) => {
            problems.push(format!("{location}: {error}"));
            false
        }
    }
}

pub use data::*;
pub use delivery::*;
pub use object::*;
pub use secrets::*;
