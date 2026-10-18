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
    let profile = if task.fresh_profile {
        task.account_id.map(|account_id| {
            let directory_key = hex::encode(Sha256::digest(account_id.as_bytes()));
            // The platform is the action's first word; an action of one
            // word is its own platform.
            let platform = match task.action.split_once('_') {
                Some((platform, _)) => platform,
                None => task.action,
            };
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
    outcome(&payload, task.action, profile)
}

/// The run document the Weles API answered, as the outcome. Every field the
/// document is declared to carry is read by name and its absence refused by
/// name: a run whose `ok` or `outputs` is missing is a Weles that answered
/// something other than a run, not a run that failed or left nothing.
fn outcome(
    payload: &Value,
    action: &str,
    profile: Option<Value>,
) -> Result<TaskOutcome, DeployError> {
    let field = |name: &str| -> Result<&Value, DeployError> {
        payload.get(name).ok_or_else(|| {
            DeployError::unreachable(format!(
                "the Weles API completed {action} and its run document carries no `{name}`: {payload}"
            ))
        })
    };
    let text = |name: &str| -> Result<String, DeployError> {
        let value = field(name)?;
        value.as_str().map(str::to_string).ok_or_else(|| {
            DeployError::unreachable(format!(
                "the Weles API completed {action} and its run document's `{name}` is not text: {value}"
            ))
        })
    };
    let ok = field("ok")?;
    let ok = ok.as_bool().ok_or_else(|| {
        DeployError::unreachable(format!(
            "the Weles API completed {action} and its run document's `ok` is not a boolean: {ok}"
        ))
    })?;
    // A run a signal ended has no exit code, and the document says so with
    // a null; a document without the key is not a run document, and a value
    // that is neither is not an exit code.
    let exit_code = match field("exitCode")? {
        Value::Null => None,
        code => Some(code.as_i64().ok_or_else(|| {
            DeployError::unreachable(format!(
                "the Weles API completed {action} and its run document's `exitCode` is not an \
                 integer: {code}"
            ))
        })?),
    };
    let outputs = field("outputs")?.clone();
    Ok(TaskOutcome {
        run_id: text("run_id")?,
        ok,
        exit_code,
        result: field("result")?.clone(),
        outputs,
        output_errors: payload.get("output_errors").cloned(),
        stdout_tail: text("stdout_tail")?,
        stderr_tail: text("stderr_tail")?,
        profile,
    })
}
