//! The bounded Cloud Billing window, opened and closed by this command alone.
//!
//! Ownership is the whole point. The window is opened only after Cloud Billing
//! has explicitly answered `billingEnabled=false`, so a window somebody else
//! opened is never adopted and never closed by mistake. A close that Cloud
//! Billing does not confirm is escalated by the caller rather than swallowed
//! — an unclosed window costs money silently.

use serde_json::{json, Value};

use crate::cli::CmdError;

const BILLING_SCOPE: &str = "https://www.googleapis.com/auth/cloud-platform";
const CLOUD_BILLING_BASE: &str = "https://cloudbilling.googleapis.com/v1/projects";

pub(super) async fn ensure_billing_disabled(project: &str) -> Result<(), CmdError> {
    let value = gcp_billing_request(reqwest::Method::GET, project, None).await?;
    match value.get("billingEnabled").and_then(Value::as_bool) {
        Some(false) => Ok(()),
        Some(true) => Err(CmdError::click(format!(
            "GCP billing for {project} is already enabled; refusing to claim ownership of a window this command did not open"
        ))),
        None => Err(CmdError::click(format!(
            "Cloud Billing response did not explicitly confirm billingEnabled=false: {value}"
        ))),
    }
}

pub(super) async fn update_gcp_billing(project: &str, account: &str) -> Result<(), CmdError> {
    let value = gcp_billing_request(
        reqwest::Method::PUT,
        project,
        Some(json!({"billingAccountName": account})),
    )
    .await?;
    let enabled = value
        .get("billingEnabled")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let landed = value
        .get("billingAccountName")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if !enabled || landed != account {
        return Err(CmdError::click(format!(
            "Cloud Billing did not confirm the requested account: {value}"
        )));
    }
    Ok(())
}

pub(super) async fn close_billing_window(project: &str) -> Result<(), CmdError> {
    let value = gcp_billing_request(
        reqwest::Method::PUT,
        project,
        Some(json!({"billingAccountName": ""})),
    )
    .await?;
    if value.get("billingEnabled").and_then(Value::as_bool) == Some(false) {
        return Ok(());
    }
    Err(CmdError::click(format!(
        "Cloud Billing did not explicitly confirm billingEnabled=false: {value}"
    )))
}

async fn gcp_billing_request(
    method: reqwest::Method,
    project: &str,
    body: Option<Value>,
) -> Result<Value, CmdError> {
    let auth = crate::skarbiec::gcp_provider()
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    let token = auth
        .token(&[BILLING_SCOPE])
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    let url = format!("{CLOUD_BILLING_BASE}/{project}/billingInfo");
    let client = reqwest::Client::new();
    let mut request = client.request(method, &url).bearer_auth(token.as_str());
    if let Some(body) = body {
        request = request.json(&body);
    }
    let response = request.send().await?;
    let status = response.status();
    let text = response.text().await?;
    if !status.is_success() {
        return Err(CmdError::click(format!(
            "Cloud Billing HTTP {status}: {text}"
        )));
    }
    Ok(serde_json::from_str(&text)?)
}

pub(super) fn combine_billing_error(open: CmdError, close: Result<(), CmdError>) -> CmdError {
    match close {
        Ok(()) => CmdError::click(format!("could not open the GCP billing window: {open}; a defensive detach request succeeded")),
        Err(close) => CmdError::click(format!("could not open the GCP billing window: {open}; CRITICAL: the defensive detach request also failed: {close}")),
    }
}
