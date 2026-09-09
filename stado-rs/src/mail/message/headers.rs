//! Case-insensitive MIME header lookup over a Gmail payload.

use serde_json::Value;

pub(super) fn header(payload: &Value, name: &str) -> String {
    payload
        .get("headers")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .find(|header| {
            header
                .get("name")
                .and_then(Value::as_str)
                .is_some_and(|value| value.eq_ignore_ascii_case(name))
        })
        .and_then(|header| header.get("value"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}
