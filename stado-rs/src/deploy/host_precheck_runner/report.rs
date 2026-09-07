//! One shape for what a runner action or reading answers, and the typed fields
//! read out of the programs the host ran.

use serde_json::{json, Value};

use super::declaration::RunnerProfile;
use crate::deploy::{host_channel, CommandOutput};
use crate::targets::ComputeTarget;

/// The failing detail a caller reports: the program's own stderr, or, when it
/// said nothing there, the last error line of what it printed. `when_silent`
/// names the failure for a program that printed neither.
pub(crate) fn command_failure(output: &CommandOutput, when_silent: &str) -> String {
    let stderr = output.stderr.trim();
    if stderr.is_empty() {
        host_channel::last_error_line(output, when_silent)
    } else {
        stderr.to_string()
    }
}

pub(crate) fn report(
    target: &ComputeTarget,
    output: &CommandOutput,
    action: &str,
    profile: &RunnerProfile,
) -> Value {
    json!({
        "target": target.name,
        "platform": target.release_platform,
        "profile": profile.name,
        "runner_kind": profile.name,
        "runner_group": profile.github_runner_group,
        "runner_labels": profile.labels_text(),
        // Read from the host's own registration record. A runner installed by
        // a build that did not record its scope reports `unrecorded` rather
        // than the declaration's desired scope.
        "runner_scope": registered_scope(&output.stdout).unwrap_or_else(|| "unrecorded".to_string()),
        "listener": listener(&output.stdout),
        // Which runner holds the host's one job slot, straight from the gate's
        // own markers: `none`, or `<account> pid=<n>`, or `<account> stale`.
        "host_job_slot": host_job_slot(&output.stdout),
        "installed": match action {
            "remove" => false,
            _ => output.ok(),
        },
        "action": action,
        "status": if output.ok() { "completed" } else { "failed" },
        "exit_code": output.code,
        "stdout": output.stdout,
        "stderr": output.stderr,
    })
}
/// The third line of `.stado/registered-runner`, which every status script
/// prints: `organization:<org>` or `repository:<org>/<name>`.
pub(crate) fn registered_scope(stdout: &str) -> Option<String> {
    stdout
        .lines()
        .map(str::trim)
        .find(|line| line.starts_with("organization:") || line.starts_with("repository:"))
        .map(str::to_string)
}

/// The listener's connection to GitHub, not merely its daemon process state.
///
/// Status programs print one `listener:` line sourced from the runner's own
/// diagnostic log. A successful restart prints `runner listener:`. Both feed
/// one typed field so callers never need to scrape script output.
fn listener(stdout: &str) -> Value {
    let state = stdout.lines().find_map(|line| {
        let line = line.trim();
        line.strip_prefix("listener: ")
            .or_else(|| line.strip_prefix("runner listener: "))
    });
    let connected = state.map(|state| {
        let lower = state.to_ascii_lowercase();
        lower.contains("listening for jobs")
            || lower.contains("running job")
            || (lower.contains("job ") && lower.contains(" completed"))
            || lower == "connected"
    });
    json!({
        "connected": connected,
        "state": state.unwrap_or("unknown"),
    })
}

/// The `host job slot:` line every status script prints — which runner holds
/// the host's one job, if any.
///
/// A host carrying several runners can run several jobs at once, and nothing
/// GitHub offers bounds that from the host's side; the gate installed beside
/// each runner does, and this is how an operator sees it working rather than
/// inferring it from a machine that stopped falling over.
fn host_job_slot(stdout: &str) -> String {
    stdout
        .lines()
        .find_map(|line| line.trim().strip_prefix("host job slot: "))
        .unwrap_or("unknown")
        .to_string()
}

pub(crate) fn unavailable_status(
    target: &ComputeTarget,
    profile: &RunnerProfile,
    error: impl ToString,
) -> Value {
    json!({
        "target": target.name,
        "platform": target.release_platform,
        "profile": profile.name,
        "runner_kind": profile.name,
        "runner_group": profile.github_runner_group,
        "runner_labels": profile.labels_text(),
        "runner_scope": Value::Null,
        "listener": {
            "connected": Value::Null,
            "state": "unavailable",
        },
        "host_job_slot": "unknown",
        "installed": Value::Null,
        "status": "unavailable",
        "error": error.to_string(),
    })
}
