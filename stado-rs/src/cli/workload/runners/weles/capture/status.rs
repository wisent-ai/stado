//! The receipt report for one `weles-capture` batch.

use serde_json::{json, Value};

use crate::cli::workload::plan::print_json;
use crate::cli::CmdError;

pub(crate) async fn weles_capture_status(
    target: &str,
    batch: &str,
    json_output: bool,
) -> Result<(), CmdError> {
    let admission = crate::deploy::weles_capture::resolve_admission(target)
        .await
        .map_err(|error| CmdError::click(format!("{target}: {error}")))?;
    let channel = crate::deploy::weles_capture::open_channel(&admission)
        .await
        .map_err(|error| CmdError::click(format!("{target}: {error}")))?;
    let batch_status = crate::deploy::weles_capture::status(&channel, batch)
        .await
        .map_err(|error| CmdError::click(format!("{target}: {error}")))?;
    let states = batch_status.captures;
    let totals = crate::deploy::weles_capture::totals(&states);
    let stored: usize = states.iter().map(|state| state.artifacts.len()).sum();
    if json_output {
        print_json(&json!({
            "kind": "weles-capture",
            "target": target,
            "batch": batch,
            "action": crate::deploy::weles_capture::CAPTURE_ACTION,
            "endpoint": admission.declared_url,
            "transport": channel.transport(),
            "artifacts_unreachable": batch_status.artifacts_unreachable,
            "actions": states.iter().map(|state| json!({
                "action_id": state.action_id,
                "site_slug": state.site_slug,
                "axis": state.axis,
                "state": state.state,
                "error": state.error,
                "artifact_prefix": state.artifact_prefix,
                "artifacts": state.artifacts,
            })).collect::<Vec<_>>(),
            "totals": totals.iter().map(|(state, count)| {
                (state.clone(), Value::from(*count))
            }).collect::<serde_json::Map<String, Value>>(),
            "artifacts_stored": stored,
        }));
    } else {
        println!(
            "{target}: batch {batch} carries {} {} action(s), {}",
            states.len(),
            crate::deploy::weles_capture::CAPTURE_ACTION,
            totals
                .iter()
                .map(|(state, count)| format!("{count} {state}"))
                .collect::<Vec<_>>()
                .join(", "),
        );
        if let Some(unreachable) = &batch_status.artifacts_unreachable {
            println!("{target}: artifact listing unreadable: {unreachable}");
        }
        for state in &states {
            println!(
                "  {:<9} {:<24} {:<13} {:>3} artifact(s)  {}{}",
                state.state,
                state.site_slug,
                state.axis,
                state.artifacts.len(),
                state.action_id,
                state
                    .error
                    .as_deref()
                    .map_or_else(String::new, |error| format!("  {error}")),
            );
        }
        println!(
            "{target}: {stored} object(s) under stado://{}/{batch}/",
            crate::deploy::weles_capture::ARTIFACT_NAMESPACE
        );
    }
    if states.is_empty() {
        return Err(CmdError::click(format!(
            "{target}: no {} action carries batch {batch}; enqueue it with `stado workload run weles-capture`",
            crate::deploy::weles_capture::CAPTURE_ACTION
        )));
    }
    Ok(())
}
