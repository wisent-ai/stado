//! Reading a host's stream report: how a failure is phrased, how one field is
//! pulled out, and the one check every command runs before it prints anything.

use serde_json::Value;

use crate::cli::CmdError;

pub(super) fn click(error: impl ToString) -> CmdError {
    CmdError::click(error.to_string())
}

pub(super) fn field(report: &Value, name: &str) -> String {
    report
        .get("fields")
        .and_then(|fields| fields.get(name))
        .map(|value| match value {
            Value::String(text) => text.clone(),
            other => other.to_string(),
        })
        .unwrap_or_else(|| "unknown".to_string())
}

pub(super) fn emitted(report: &Value, json: bool, expected: &str) -> Result<bool, CmdError> {
    if report.get("status").and_then(Value::as_str) != Some(expected) {
        return Err(CmdError::click(format!(
            "stream operation did not reach {expected}: {report}"
        )));
    }
    if json {
        println!("{}", serde_json::to_string_pretty(report)?);
        return Ok(false);
    }
    Ok(true)
}
