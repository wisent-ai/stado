//! The scalar renderer both billing output paths format through: `show`'s
//! per-provider lines and `watch`'s per-poll tables read the same JSON
//! sections, so an absent field has to print the same word in both.

use serde_json::Value;

pub(super) fn text(value: Option<&Value>) -> String {
    match value {
        None | Some(Value::Null) => "unknown".to_string(),
        Some(Value::String(value)) => value.clone(),
        Some(value) => value.to_string(),
    }
}
