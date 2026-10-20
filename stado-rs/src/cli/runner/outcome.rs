//! The verdict of one runner lifecycle report: the line an operator reads,
//! and whether the command it reports succeeded.

use serde_json::Value;

use super::{connected, print_json, text};
use crate::cli::CmdError;

pub(super) fn render_lifecycle(report: &Value, json: bool) -> Result<(), CmdError> {
    if json {
        print_json(report);
    } else {
        println!(
            "{}: {} runner {}; scope {}; listener {}; host job slot {}",
            text(report.get("target")),
            text(report.get("profile")),
            text(report.get("action")),
            text(report.get("runner_scope")),
            connected(report),
            text(report.get("host_job_slot")),
        );
    }
    lifecycle_outcome(report).map_err(|error| error.machine_readable(json))
}

/// A report that ran no host command (a restart or runtime repair) carries
/// no `exit_code` key and is judged by its route. One that ran it states the
/// command's own verdict as `status`; its exit code is null when a signal
/// ended it, which is never read as a zero and taken for success.
fn lifecycle_outcome(report: &Value) -> Result<(), CmdError> {
    let Some(code) = report.get("exit_code") else {
        return route_outcome(report);
    };
    if report.get("status").and_then(Value::as_str) == Some("completed") {
        return route_outcome(report);
    }
    let detail = match (report.get("error").and_then(Value::as_str), code.as_i64()) {
        (Some(error), _) => error.to_string(),
        (None, Some(code)) => format!(
            "{}: the runner command exited {code}: {}",
            text(report.get("target")),
            text(report.get("stderr"))
        ),
        (None, None) => format!(
            "{}: the runner command ended without an exit code, so it did not finish: {}",
            text(report.get("target")),
            text(report.get("stderr"))
        ),
    };
    Err(CmdError::click(detail).stating(crate::primitives::failure::FailureCode::InfraDown))
}

/// A runner whose Brama route differs from the fleet declaration is
/// configuration drift, not an unknown failure.
fn route_outcome(report: &Value) -> Result<(), CmdError> {
    let Some(route) = report.get("brama_route") else {
        return Ok(());
    };
    if route.get("matches").and_then(Value::as_bool) == Some(true) {
        return Ok(());
    }
    let detail = match route.get("detail").and_then(Value::as_str) {
        Some(detail) => detail,
        None => "the runner's Brama route does not match the fleet declaration",
    };
    Err(
        CmdError::click(format!("{}: {detail}", text(report.get("target"))))
            .stating(crate::primitives::failure::FailureCode::Config),
    )
}
