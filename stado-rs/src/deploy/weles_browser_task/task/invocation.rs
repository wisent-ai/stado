//! The invocation: the held-open submission that carries one task through to
//! the result its run produced.

use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use super::receipts::TaskOutcome;
use super::BrowserTask;
use crate::deploy::{weles_capture, DeployError};

/// Submit the task and carry it through to its result.
///
/// Synchronous by construction: [`weles_capture::observe_action_payload`]
/// holds the request open for the run and returns what the run produced, so
/// this command reports an outcome rather than a queue receipt. A caller that
/// only wanted a receipt would have to poll the action log, and a browser
/// flow whose result nobody read is how a sign-in fails unnoticed.
pub async fn submit(
    target: &str,
    task: &BrowserTask<'_>,
    flow_name: Option<&str>,
    credential_deferred: &[Value],
) -> Result<TaskOutcome, DeployError> {
    let admission = weles_capture::resolve_admission(target).await?;
    let channel = weles_capture::open_channel(&admission).await?;
    let payload = weles_capture::observe_action_payload(
        &channel,
        task.action,
        task.params_with(flow_name, credential_deferred),
        task.account_id,
        task.fresh_profile,
    )
    .await?;
    let run_id = payload
        .get("run_id")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let profile = if task.fresh_profile {
        task.account_id.map(|account_id| {
            let directory_key = hex::encode(Sha256::digest(account_id.as_bytes()));
            let platform = task.action.split('_').next().unwrap_or("unknown");
            json!({
                "mode": "fresh",
                "account_id": account_id,
                "directory_key": directory_key,
                "directory": format!(
                    "$HOME/.local/state/weles/browser-profiles/{platform}/chromium/{directory_key}"
                ),
            })
        })
    } else {
        None
    };
    Ok(TaskOutcome {
        ok: payload.get("ok").and_then(Value::as_bool).unwrap_or(false),
        exit_code: payload.get("exitCode").and_then(Value::as_i64),
        result: payload.get("result").cloned().unwrap_or(Value::Null),
        stdout_tail: payload
            .get("stdout_tail")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        stderr_tail: payload
            .get("stderr_tail")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        run_id,
        profile,
    })
}
