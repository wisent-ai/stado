//! The scalar renderer both billing output paths format through: `show`'s
//! per-provider lines and `watch`'s per-poll tables read the same JSON
//! sections, so an absent field has to print the same words in both: "not
//! reported", which says what happened, rather than "unknown", which would
//! read as a value the provider gave.

use serde_json::Value;

pub(super) fn text(value: Option<&Value>) -> String {
    match value {
        None | Some(Value::Null) => "not reported".to_string(),
        Some(Value::String(value)) => value.clone(),
        Some(value) => value.to_string(),
    }
}
