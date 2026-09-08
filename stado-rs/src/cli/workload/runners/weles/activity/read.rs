//! Run the activity source on one host and render what it printed.

use serde_json::{json, Value};

use crate::cli::workload::plan::print_json;
use crate::cli::CmdError;
use crate::deploy::host_channel;
use crate::targets::ComputeTarget;

use super::source::WELES_ACTIVITY_SOURCE;

/// The marker [`WELES_ACTIVITY_SOURCE`] prefixes to its one JSON line, so a
/// login shell's own greeting cannot be mistaken for the report.
const WELES_ACTIVITY_MARKER: &str = "STADO-WELES-ACTIVITY ";

/// Run [`WELES_ACTIVITY_SOURCE`] on one host with the host's own node, and
/// hand back what it printed.
///
/// The run limit and API port are the host's environment or the defaults the
/// retired wrapper carried, resolved on the host so an operator's local
/// environment cannot steer a remote read.
async fn read_weles_activity(
    resolved: &ComputeTarget,
    runner: &crate::deploy::Runner,
) -> Result<String, crate::deploy::DeployError> {
    use crate::deploy::host_channel;
    let mut node = None;
    for candidate in ["/opt/homebrew/bin/node", "/usr/local/bin/node"] {
        if host_channel::remote_test(resolved, &format!("-x {candidate}"), runner).await? {
            node = Some(candidate);
            break;
        }
    }
    let Some(node) = node else {
        return Err(crate::deploy::DeployError(
            "Node.js is unavailable on this host".to_string(),
        ));
    };
    let environment = host_channel::run_command(
        resolved,
        "printf '%s %s' \"${WELES_ACTIVITY_RUN_LIMIT:-40}\" \"${WELES_API_PORT:-8788}\"",
        runner,
    )
    .await?;
    if !environment.ok() {
        return Err(crate::deploy::DeployError(host_channel::last_error_line(
            &environment,
            "the host's Weles environment could not be read",
        )));
    }
    let mut values = environment.stdout.split_whitespace();
    let limit = values.next().unwrap_or("40");
    let port = values.next().unwrap_or("8788");
    let output = host_channel::run_program_with_stdin(
        resolved,
        &[node, "-", limit, port],
        WELES_ACTIVITY_SOURCE,
        runner,
    )
    .await?;
    if !output.ok() {
        return Err(crate::deploy::DeployError(host_channel::last_error_line(
            &output,
            "the Weles activity read did not complete",
        )));
    }
    Ok(output.stdout)
}
pub(crate) async fn weles_activity(target: &str, json_output: bool) -> Result<(), CmdError> {
    let runner = crate::deploy::production_runner();
    let resolved = host_channel::canonical_target(target)
        .await
        .map_err(|error| {
            CmdError::click(format!("{target}: cannot read Weles activity: {error}"))
        })?;
    let output = read_weles_activity(&resolved, &runner)
        .await
        .map_err(|error| {
            CmdError::click(format!("{target}: cannot read Weles activity: {error}"))
        })?;
    let document = output
        .lines()
        .filter_map(|line| line.trim().strip_prefix(WELES_ACTIVITY_MARKER))
        .next_back()
        .ok_or_else(|| {
            CmdError::click(format!(
                "{target}: the Weles activity read printed no report line; inspect the host runtime"
            ))
        })?;
    let mut report: Value = serde_json::from_str(document).map_err(|error| {
        CmdError::click(format!(
            "{target}: the Weles activity report is not readable JSON: {error}"
        ))
    })?;
    if let Some(object) = report.as_object_mut() {
        object.insert("kind".to_string(), json!("weles-activity"));
    }
    if json_output {
        print_json(&report);
        return Ok(());
    }
    let worker = &report["worker"];
    println!(
        "{target}: worker {} staged, {} newest installed, API {} on {}",
        worker["staged_release"].as_str().unwrap_or("unknown"),
        worker["newest_release"].as_str().unwrap_or("unknown"),
        if report["api"]["listening"].as_bool().unwrap_or_default() {
            "answering"
        } else {
            "silent"
        },
        report["api"]["endpoint"]
            .as_str()
            .unwrap_or("unknown endpoint"),
    );
    let runs = report["runs"].as_array().map_or(&[][..], Vec::as_slice);
    println!(
        "{target}: {} recorded run(s), {} newest below",
        report["run_total"].as_u64().unwrap_or_default(),
        runs.len()
    );
    for run in runs {
        println!(
            "  {:<10} {:<22} {:<38} {}",
            run["status"].as_str().unwrap_or("unknown"),
            run["action"].as_str().unwrap_or("unknown action"),
            run["id"].as_str().unwrap_or("-"),
            run["updated_at"].as_str().unwrap_or("-"),
        );
    }
    Ok(())
}
