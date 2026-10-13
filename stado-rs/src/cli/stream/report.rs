//! Reading a host's stream report: how a failure is phrased, how one field is
//! pulled out, and the one check every command runs before it prints anything.

use serde_json::Value;

use crate::cli::CmdError;

/// The text a field shows to a person, or a word saying the host did not
/// report it: a value nobody stated is never shown as one.
pub(super) fn field(report: &Value, name: &str) -> String {
    match report.get("fields").and_then(|fields| fields.get(name)) {
        Some(Value::String(text)) => text.clone(),
        Some(other) => other.to_string(),
        None => NOT_REPORTED.to_string(),
    }
}

/// What [`field`] shows for a field the host's report does not carry.
pub(super) const NOT_REPORTED: &str = "not reported";

/// One field a decision is made on: a report without it is refused naming
/// the field and quoting the report, because a decision on a missing value
/// would be made on a word nobody stated.
pub(super) fn required_field(report: &Value, name: &str) -> Result<String, CmdError> {
    match report.get("fields").and_then(|fields| fields.get(name)) {
        Some(Value::String(text)) => Ok(text.clone()),
        Some(other) => Ok(other.to_string()),
        None => Err(CmdError::click(format!(
            "the host's stream report carries no {name} field: {report}"
        ))
        .stating(crate::primitives::failure::FailureCode::InfraDown)),
    }
}

pub(super) fn emitted(report: &Value, json: bool, expected: &str) -> Result<bool, CmdError> {
    if report.get("status").and_then(Value::as_str) != Some(expected) {
        return Err(CmdError::click(format!(
            "stream operation did not reach {expected}: {report}"
        ))
        .stating(crate::primitives::failure::FailureCode::InfraDown));
    }
    if json {
        println!("{}", serde_json::to_string_pretty(report)?);
        return Ok(false);
    }
    Ok(true)
}
