//! Cloudflare response payload readers: the result array every list endpoint
//! returns, its required string fields, and the one exact active zone.

use serde_json::Value;

use crate::cli::CmdError;

pub(in crate::cli::cloudflare) fn exact_zone_id(
    payload: &Value,
    zone: &str,
) -> Result<String, CmdError> {
    let zones = result_array(payload, "Cloudflare zone lookup")?;
    let exact: Vec<&Value> = zones
        .iter()
        .filter(|candidate| candidate.get("name").and_then(Value::as_str) == Some(zone))
        .collect();
    if exact.len() != 1 {
        return Err(CmdError::click(format!(
            "Cloudflare returned {} active exact zones named {zone:?}; expected one",
            exact.len()
        )));
    }
    required_string(exact[0], "id")
}

pub(in crate::cli::cloudflare) fn result_array<'a>(
    payload: &'a Value,
    context: &str,
) -> Result<&'a Vec<Value>, CmdError> {
    payload
        .get("result")
        .and_then(Value::as_array)
        .ok_or_else(|| CmdError::click(format!("{context} result is not an array")))
}

pub(in crate::cli::cloudflare) fn required_string(
    value: &Value,
    field: &str,
) -> Result<String, CmdError> {
    value
        .get(field)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .ok_or_else(|| CmdError::click(format!("Cloudflare response field {field:?} is required")))
}
