//! CLI rendering for the isolated GitHub pre-check runner lifecycle.

use serde_json::Value;

use super::CmdError;

fn cell(value: Option<&Value>) -> &str {
    value.and_then(Value::as_str).unwrap_or("-")
}

async fn render(target: &str, action: &str, json_output: bool) -> Result<(), CmdError> {
    let report = match action {
        "install" => crate::deploy::host_precheck_runner::install(target).await,
        "status" => crate::deploy::host_precheck_runner::status(target).await,
        "remove" => crate::deploy::host_precheck_runner::remove(target).await,
        _ => unreachable!("fixed precheck runner action"),
    }
    .map_err(|error| CmdError::click(error.to_string()))?;
    if json_output {
        println!(
            "{}",
            crate::deploy::host_recovery::to_sorted_pretty(&report)
        );
        return Ok(());
    }
    println!(
        "{}: precheck runner {} on {} ({})",
        cell(report.get("target")),
        action,
        cell(report.get("platform")),
        cell(report.get("runner_group"))
    );
    let stdout = report.get("stdout").and_then(Value::as_str).unwrap_or("");
    if !stdout.is_empty() {
        print!("{stdout}");
    }
    let stderr = report.get("stderr").and_then(Value::as_str).unwrap_or("");
    if !stderr.is_empty() {
        eprint!("{stderr}");
    }
    Ok(())
}

/// Install or reconcile the isolated GitHub pre-check runner on TARGET.
pub async fn install(target: &str, json: bool) -> Result<(), CmdError> {
    render(target, "install", json).await
}

/// Read the installed runner service, identity and network boundary on TARGET.
pub async fn status(target: &str, json: bool) -> Result<(), CmdError> {
    render(target, "status", json).await
}

/// Deregister and remove the isolated GitHub pre-check runner from TARGET.
pub async fn remove(target: &str, json: bool) -> Result<(), CmdError> {
    render(target, "remove", json).await
}
