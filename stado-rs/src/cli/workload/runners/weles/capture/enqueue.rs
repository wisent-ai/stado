//! Enqueue one `weles-capture` batch, all of it or none of it.

use serde_json::json;

use crate::cli::workload::plan::print_json;
use crate::cli::CmdError;

pub(crate) async fn run_weles_capture(
    target: &str,
    plan_path: &str,
    json_output: bool,
) -> Result<(), CmdError> {
    // Parsing validates the entire plan before the admission endpoint is
    // resolved or contacted. Enqueue is therefore all-or-nothing.
    let plan = crate::deploy::weles_capture::parse_plan(plan_path, target, None)
        .map_err(|error| CmdError::usage(error.to_string()))?;
    let admission = crate::deploy::weles_capture::resolve_admission(target)
        .await
        .map_err(|error| CmdError::click(format!("{target}: {error}")))?;
    let channel = crate::deploy::weles_capture::open_channel(&admission)
        .await
        .map_err(|error| CmdError::click(format!("{target}: {error}")))?;
    let accepted = crate::deploy::weles_capture::enqueue(&channel, &plan)
        .await
        .map_err(|error| CmdError::click(format!("{target}: {error}")))?;
    if json_output {
        print_json(&json!({
            "kind": "weles-capture",
            "target": target,
            "batch": plan.batch,
            "action": crate::deploy::weles_capture::CAPTURE_ACTION,
            "endpoint": admission.declared_url,
            "transport": channel.transport(),
            "admission_token": channel.token_state(),
            "enqueued": accepted.len(),
            "actions": accepted.iter().map(|action| json!({
                "action_id": action.action_id,
                "site_slug": action.site_slug,
                "axis": action.axis,
                "artifact_prefix": action.artifact_prefix,
            })).collect::<Vec<_>>(),
            "status": "enqueued",
        }));
    } else {
        println!(
            "{target}: enqueued {} {} action(s) for batch {} on {}",
            accepted.len(),
            crate::deploy::weles_capture::CAPTURE_ACTION,
            plan.batch,
            admission.declared_url,
        );
        for action in &accepted {
            println!(
                "  {:<38} {:<24} {:<13} {}",
                action.action_id, action.site_slug, action.axis, action.artifact_prefix,
            );
        }
    }
    Ok(())
}
