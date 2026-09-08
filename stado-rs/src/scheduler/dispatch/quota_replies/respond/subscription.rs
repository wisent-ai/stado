//! The subscription identity the escalation body cites: the account id
//! and the quotaId that proves the subscription is credit-funded.

use super::super::runner::{az, AzRunner, RepliesError};

/// Python `_subscription_id`.
pub(super) fn subscription_id(runner: &dyn AzRunner) -> Result<String, RepliesError> {
    let r = az(runner, &["account", "show", "--query", "id"])?;
    Ok(r.as_str().unwrap_or("").to_string())
}

/// quotaId proves the subscription is sponsored (Sponsored_*).
/// az account show does NOT include subscriptionPolicies by default,
/// so hit management.azure.com via `az rest` directly.
/// Python `_subscription_quota_id`.
pub(super) fn subscription_quota_id(runner: &dyn AzRunner) -> Result<String, RepliesError> {
    let sub = subscription_id(runner)?;
    if sub.is_empty() {
        return Ok(String::new());
    }
    let r = az(
        runner,
        &[
            "rest",
            "--method",
            "GET",
            "--uri",
            &format!("https://management.azure.com/subscriptions/{sub}?api-version=2022-12-01"),
            "--query",
            "subscriptionPolicies.quotaId",
        ],
    )?;
    Ok(r.as_str().unwrap_or("").to_string())
}
