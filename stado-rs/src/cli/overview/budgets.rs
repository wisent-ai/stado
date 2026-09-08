//! The budgets section: the limits the fleet is held to, from the autonomy
//! policy's own forecast and from GCP Billing Budgets.
//!
//! `authorized_json` is the one HTTP shape both Google reads need — a bearer
//! GET whose failure becomes a string rather than an error that would sink
//! the whole overview.

use serde_json::{json, Value};

use crate::queue::JobStorage;

const CLOUD_BILLING_BASE: &str = "https://cloudbilling.googleapis.com";
const BILLING_BUDGETS_BASE: &str = "https://billingbudgets.googleapis.com";
const CLOUD_PLATFORM_SCOPE: &str = "https://www.googleapis.com/auth/cloud-platform";

async fn authorized_json(
    client: &reqwest::Client,
    url: &str,
    token: &str,
) -> Result<Value, String> {
    let response = client
        .get(url)
        .bearer_auth(token)
        .send()
        .await
        .map_err(|err| err.to_string())?;
    let status = response.status();
    let text = response.text().await.map_err(|err| err.to_string())?;
    if !status.is_success() {
        return Err(format!("HTTP {status}: {text}"));
    }
    serde_json::from_str(&text).map_err(|err| err.to_string())
}

/// The budgets the fleet is actually held to.
///
/// The autonomy policy's `budgets` block is what gates new cloud placement
/// (docs: costs), and the cycle persists its verdict as the cost forecast; that
/// is the figure an operator needs beside the burn. GCP Billing Budgets are
/// read only while `gcp` is an enabled billing provider: the historical
/// project's billing is detached on purpose, and asking it for budgets from
/// every overview turned the whole section into "token request failed".
pub(super) async fn read_budgets(store: &JobStorage) -> Value {
    let forecast: Option<Value> =
        crate::autonomy::storage::read_json::<Value>(store, "state/autonomy/cost/forecast.json")
            .await
            .ok()
            .flatten();
    let policy = match &forecast {
        Some(forecast) => json!({
            "status": "ok",
            "source": "autonomy policy, via state/autonomy/cost/forecast.json",
            "created_at": forecast.get("created_at").cloned().unwrap_or(Value::Null),
            "hourly_usd": forecast.get("hourly_budget_usd").cloned().unwrap_or(Value::Null),
            "daily_usd": forecast.get("daily_budget_usd").cloned().unwrap_or(Value::Null),
            "monthly_usd": forecast.get("monthly_budget_usd").cloned().unwrap_or(Value::Null),
            "current_hourly_usd": forecast.get("current_hourly_usd").cloned().unwrap_or(Value::Null),
            "end_of_month_usd": forecast.get("end_of_month_usd").cloned().unwrap_or(Value::Null),
            "budget_exceeded": forecast.get("budget_exceeded").cloned().unwrap_or(Value::Null),
            "credit_runway_days": forecast.get("credit_runway_days").cloned().unwrap_or(Value::Null),
        }),
        None => json!({
            "status": "unavailable",
            "detail": "no cost forecast persisted yet; run `stado optimize run`",
        }),
    };
    let gcp = if crate::config::billing_providers()
        .iter()
        .any(|provider| provider == "gcp")
    {
        read_gcp_budgets().await
    } else {
        json!({
            "status": "detached",
            "detail": "gcp is not an enabled billing provider (billing.providers)",
        })
    };
    json!({ "policy": policy, "gcp": gcp })
}

async fn read_gcp_budgets() -> Value {
    let auth = match crate::skarbiec::gcp_provider().await {
        Ok(auth) => auth,
        Err(err) => return json!({"status": "error", "detail": err.to_string()}),
    };
    let token = match auth.token(&[CLOUD_PLATFORM_SCOPE]).await {
        Ok(token) => token,
        Err(err) => return json!({"status": "error", "detail": err.to_string()}),
    };
    let client = reqwest::Client::new();
    let billing_info_url = format!(
        "{CLOUD_BILLING_BASE}/v1/projects/{}/billingInfo",
        crate::config::project()
    );
    let billing_info = match authorized_json(&client, &billing_info_url, token.as_str()).await {
        Ok(value) => value,
        Err(err) => return json!({"status": "error", "detail": err}),
    };
    let Some(account) = billing_info
        .get("billingAccountName")
        .and_then(Value::as_str)
    else {
        return json!({"status": "unavailable", "detail": "project has no billing account"});
    };
    let budgets_url = format!("{BILLING_BUDGETS_BASE}/v1/{account}/budgets");
    match authorized_json(&client, &budgets_url, token.as_str()).await {
        Ok(value) => json!({
            "status": "ok",
            "billing_account": account,
            "budgets": value.get("budgets").cloned().unwrap_or_else(|| json!([])),
        }),
        Err(err) => json!({"status": "error", "billing_account": account, "detail": err}),
    }
}
