//! The subscription identity the escalation body cites: the account id
//! and the quotaId that proves the subscription is credit-funded.

use super::super::runner::{az, AzRunner, RepliesError};

/// Python `_subscription_id`. The id is cited in every reply posted to
/// Microsoft, so an answer that carries none is refused rather than read as
/// an empty id.
pub(super) fn subscription_id(runner: &dyn AzRunner) -> Result<String, RepliesError> {
    let r = az(runner, &["account", "show", "--query", "id"])?;
    match r.as_str().filter(|id| !id.is_empty()) {
        Some(id) => Ok(id.to_string()),
        None => Err(RepliesError::Config(format!(
            "Azure answered the subscription lookup without an id: {r}"
        ))),
    }
}

/// quotaId proves the subscription is sponsored (Sponsored_*).
/// az account show does NOT include subscriptionPolicies by default,
/// so hit management.azure.com via `az rest` directly.
/// Python `_subscription_quota_id`.
pub(super) fn subscription_quota_id(runner: &dyn AzRunner) -> Result<String, RepliesError> {
    let sub = subscription_id(runner)?;
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
    match r.as_str().filter(|quota| !quota.is_empty()) {
        Some(quota) => Ok(quota.to_string()),
        None => Err(RepliesError::Config(format!(
            "Azure answered subscription {sub} without subscriptionPolicies.quotaId, so the escalation cannot cite what funds it: {r}"
        ))),
    }
}
