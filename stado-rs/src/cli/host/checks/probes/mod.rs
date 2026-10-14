//! Read-only host diagnostics and the rendering they share.

pub(in crate::cli::host) mod gates;
pub(in crate::cli::host) mod inventory;
pub(in crate::cli::host) mod vitals;

use serde_json::Value;

use crate::cli::CmdError;

// ---------------------------------------------------------------------------
// stado.wisent.com/docs/missing-commands items two through six
//
// Each of these is a thin shell over one `crate::deploy` module: resolve,
// run through the shared ssh channel, then either print the report as JSON
// or render it. A non-zero remote exit is a click error carrying the remote's
// own last line, so the shell exit status matches the health of the host.
// ---------------------------------------------------------------------------

/// Print `report` as sorted-keys JSON.
pub(in crate::cli::host) fn print_json(report: &Value) {
    println!("{}", crate::deploy::host_recovery::to_sorted_pretty(report));
}

/// A report's `error` field, or its `status`, as a click error.
///
/// Returns `Ok` when the report is healthy. `expected` is the `status`
/// value that means "this command did what it was asked to do"; anything
/// else is a failure the exit status has to reflect.
pub(in crate::cli::host) fn report_outcome(report: &Value, expected: &str) -> Result<(), CmdError> {
    let status = report.get("status").and_then(Value::as_str);
    if status == Some(expected) {
        return Ok(());
    }
    // A report that names no status is said as such, not as status "".
    let detail = match (report.get("error").and_then(Value::as_str), status) {
        (Some(error), _) => error.to_string(),
        (None, Some(status)) => format!("host reported status {status:?}"),
        (None, None) => format!("host reported no status and no error: {report}"),
    };
    Err(CmdError::click(detail).stating(crate::primitives::failure::FailureCode::InfraDown))
}

/// A JSON value as one table cell: strings bare, a missing or null value as
/// "not reported", anything else in its JSON spelling. A dash read as a value
/// the host had reported.
pub(in crate::cli::host) fn cell(value: Option<&Value>) -> String {
    match value {
        None | Some(Value::Null) => "not reported".to_string(),
        Some(Value::String(text)) => text.clone(),
        Some(other) => other.to_string(),
    }
}
