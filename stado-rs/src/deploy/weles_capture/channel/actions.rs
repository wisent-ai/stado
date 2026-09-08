//! The browser and runtime invocations one open channel carries: the `/run`
//! body, the account binding that keys the profile, and the action log.

use serde_json::{json, Value};

use super::super::{QUERY_LIMIT, QUERY_ROUTE, RUN_ROUTE, RUN_TIMEOUT};
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
                "timeout_ms": RUN_TIMEOUT.as_millis(),
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
        .observe_run(&run_request(action, params, account_id, fresh_profile))
        .await
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
        "timeout_ms": RUN_TIMEOUT.as_millis(),
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

/// Read the newest recorded row for one fixed action.
pub async fn latest_action_log(
    channel: &Channel,
    action: &str,
) -> Result<Option<Value>, DeployError> {
    let data = match channel
        .call(
            QUERY_ROUTE,
            &json!({ "action": action, "limit": QUERY_LIMIT }),
        )
        .await
    {
        Ok(data) => data,
        Err(error)
            if error
                .0
                .contains("refused /v1/echo/action-logs/query with 404 Not Found") =>
        {
            return Ok(None);
        }
        Err(error) => return Err(error),
    };
    let logs = data
        .get("logs")
        .and_then(Value::as_array)
        .ok_or_else(|| DeployError("the Weles admission API returned no action log".to_string()))?;
    Ok(logs.first().cloned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pinned_account_reaches_the_api_as_the_binding_that_keys_the_profile() {
        let request = run_request(
            "generic_browser_task",
            json!({"url": "https://x.test/"}),
            Some("oko-calendar"),
            false,
        );
        assert_eq!(request["account_id"], json!("oko-calendar"));
        // Not a fresh profile: the point of pinning is to REUSE the directory
        // that identity already has, cookies and signed-in session included.
        assert_eq!(request.get("fresh_profile"), None);
    }

    #[test]
    fn a_run_without_an_account_carries_no_binding_at_all() {
        let request = run_request("generic_browser_task", json!({}), None, false);
        assert_eq!(request.get("account_id"), None);
        assert_eq!(request["action"], json!("generic_browser_task"));
    }

    #[test]
    fn a_fresh_profile_still_names_the_account_its_new_directory_belongs_to() {
        // The API refuses `fresh_profile` without one, so the two travel together.
        let request = run_request(
            "generic_browser_task",
            json!({}),
            Some("stado-fresh-profile-1"),
            true,
        );
        assert_eq!(request["fresh_profile"], json!(true));
        assert_eq!(request["account_id"], json!("stado-fresh-profile-1"));
    }

    #[test]
    fn an_account_id_that_could_not_be_a_directory_key_is_refused_here() {
        for bad in [
            "",
            "has space",
            "slash/inside",
            "quote\"inside",
            "$(command)",
        ] {
            let said = checked_account_id(bad).unwrap_err().to_string();
            assert!(said.contains("must be 1-128 characters"), "{bad:?}: {said}");
        }
        assert_eq!(
            checked_account_id("oko-calendar.lukasz_1").unwrap(),
            "oko-calendar.lukasz_1"
        );
    }
}
