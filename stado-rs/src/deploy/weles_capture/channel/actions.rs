//! The browser and runtime invocations one open channel carries: the `/run`
//! body and the account binding that keys the profile.

use serde_json::{json, Value};

use super::super::RUN_ROUTE;
use super::Channel;
use crate::deploy::DeployError;

/// Run one fixed non-capture action through Weles's synchronous API and retain
/// its complete redacted result.
pub async fn run_action_payload(
    channel: &Channel,
    action: &str,
    params: Value,
) -> Result<Value, DeployError> {
    channel
        .call(
            RUN_ROUTE,
            &json!({
                "action": action,
                "params": params,
                "creds": "redact",
            }),
        )
        .await
}

/// Run one fixed browser action and keep its run envelope even when the
/// trajectory itself fails after navigation. The envelope carries the run id
/// needed to read Weles's completed browser diagnostics.
pub async fn observe_action_payload(
    channel: &Channel,
    action: &str,
    params: Value,
    account_id: Option<&str>,
    fresh_profile: bool,
) -> Result<Value, DeployError> {
    channel
        .run_outcome(&run_request(action, params, account_id, fresh_profile))
        .await
        .map(|(payload, _failure)| payload)
}

/// The `/run` body, built where it can be read in a test.
///
/// `account_id` is the account binding the Weles API turns into `ACCOUNT_ID`
/// in the trajectory's environment, which is what keys the browser profile
/// directory. Absent it, every run is a new device to the site being driven -
/// which is how five sign-ins in a row each met a first-visit risk check, one
/// of them a passkey demand.
pub fn run_request(
    action: &str,
    params: Value,
    account_id: Option<&str>,
    fresh_profile: bool,
) -> Value {
    let mut request = json!({
        "action": action,
        "params": params,
        "creds": "redact",
    });
    if let Some(account_id) = account_id {
        request["account_id"] = json!(account_id);
    }
    if fresh_profile {
        request["fresh_profile"] = json!(true);
    }
    request
}

/// One account id, refused unless it is safe to use as an identity and a
/// directory key.
///
/// Weles hashes it for the profile directory and the API echoes it into the
/// child environment, so anything outside this alphabet is refused here rather
/// than resolved on the host.
pub fn checked_account_id(account_id: &str) -> Result<&str, DeployError> {
    if account_id.is_empty()
        || account_id.len() > 128
        || !account_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
    {
        return Err(DeployError(format!(
            "account id {account_id:?} must be 1-128 characters of letters, digits, '-', '_' or '.'"
        )));
    }
    Ok(account_id)
}

/// Run one non-capture action through Weles's synchronous API.
///
/// Callers still name a fixed action in product code; this function does not
/// expose an arbitrary-action CLI.
pub async fn run_action(
    channel: &Channel,
    action: &str,
    params: Value,
) -> Result<String, DeployError> {
    let data = run_action_payload(channel, action, params).await?;
    data.get("run_id")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
        .map(str::to_string)
        .ok_or_else(|| {
            DeployError("the Weles API completed the action and returned no run id".to_string())
        })
}
