//! The `weles-image-inspect` workload: what a public page's images did in a
//! real browser on a host.

use serde_json::{json, Value};

use crate::cli::workload::plan::print_json;
use crate::cli::CmdError;

pub(crate) async fn run_weles_image_inspect(
    target: &str,
    source_url: &str,
    json_output: bool,
) -> Result<(), CmdError> {
    let parsed = url::Url::parse(source_url)
        .map_err(|error| CmdError::usage(format!("workload plan url is not a URL: {error}")))?;
    if parsed.scheme() != "https"
        || parsed.host_str().is_none()
        || !parsed.username().is_empty()
        || parsed.password().is_some()
    {
        return Err(CmdError::usage(
            "weles-image-inspect plan url must be an HTTPS URL without embedded credentials",
        ));
    }
    let source_url = parsed.to_string();
    let host = parsed.host_str().ok_or_else(|| {
        CmdError::usage("weles-image-inspect plan url declares no host; add it to the URL")
    })?;
    let objective = "Inspect this public page without signing in or changing any user or application state. Scroll through the whole page to trigger lazy-loaded media. Inspect every rendered img element and report its currentSrc URL host and pathname, complete flag, naturalWidth and naturalHeight. Inspect PerformanceResourceTiming entries for image resources and /api/stado/object requests, including responseStatus where Chromium exposes it. Count loaded and failed images, count /api/stado/object image URLs, list every failed URL or HTTP status, and list any visible image-error placeholder text and the affected card or room name. Return one concise JSON object containing final_url, rendered_images, loaded_images, failed_images, stado_object_images, failed_resources, and visible_placeholders.";
    let admission = crate::deploy::weles_capture::resolve_admission(target)
        .await
        .map_err(|error| CmdError::click(format!("{target}: {error}")))?;
    let channel = crate::deploy::weles_capture::open_channel(&admission)
        .await
        .map_err(|error| CmdError::click(format!("{target}: {error}")))?;
    let result = crate::deploy::weles_capture::observe_action_payload(
        &channel,
        "generic_browser_task",
        json!({
            "url": source_url.as_str(),
            "objective": objective,
            "flow_name": format!("stado-image-inspection:{host}"),
            "session_label": format!("stado-image-inspection-{host}"),
            "proxy": "none",
            "headless": true,
            "constraints": {
                "read_only": true,
                "no_login": true,
                "no_mutation": true,
            },
        }),
        None,
        false,
    )
    .await
    .map_err(|error| CmdError::click(format!("{target}: {error}")))?;
    let run_id = result
        .get("run_id")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            CmdError::click(format!(
                "{target} returned no Weles diagnostic run id; inspect the Weles admission report"
            ))
        })?;
    let diagnostics = crate::deploy::weles_capture::image_diagnostics(&channel, run_id)
        .await
        .map_err(|error| CmdError::click(format!("{target}: {error}")))?;
    let task_result = result.get("result").cloned().unwrap_or(Value::Null);
    let report = json!({
        "kind": "weles-image-inspect",
        "target": target,
        "source_url": source_url.as_str(),
        "action": "generic_browser_task",
        "endpoint": admission.declared_url,
        "transport": channel.transport(),
        "admission_token": channel.token_state(),
        "browser_run": {
            "run_id": run_id,
            "trajectory_ok": result.get("ok").and_then(Value::as_bool).unwrap_or(false),
            "exit_code": result.get("exitCode"),
            "final_url": task_result.get("final_url"),
            "trajectory_error": task_result.get("error"),
        },
        "images": diagnostics,
    });
    if json_output {
        print_json(&report);
    } else {
        println!(
            "{target}: inspected {} through {}",
            report["source_url"].as_str().unwrap_or(source_url.as_str()),
            admission.declared_url,
        );
        print_json(&diagnostics);
    }
    Ok(())
}
