//! Azure arm: the Microsoft.Quota create_or_update body, the
//! Microsoft.Compute scope it hangs under, and the ARM LRO submission
//! (injectable-client half plus the AZURE_SUBSCRIPTION_ID guard).

use serde_json::{json, Value};

use crate::providers::azure::{ArmClient, AzureError};

/// Microsoft.Quota API version used by the Python azure-mgmt-quota SDK.
pub const AZURE_QUOTA_API_VERSION: &str = "2023-02-01";

/// The Microsoft.Quota create_or_update body (Python
/// `create_quota_request`).
pub fn azure_quota_body(family_name: &str, new_limit: i64) -> Value {
    json!({
        "properties": {
            "limit": {"limitObjectType": "LimitValue", "value": new_limit},
            "name": {"value": family_name},
            "resourceType": "dedicated",
        }
    })
}

/// The Microsoft.Compute scope the quota resource hangs under.
pub fn azure_quota_scope(subscription: &str, location: &str) -> String {
    format!("subscriptions/{subscription}/providers/Microsoft.Compute/locations/{location}")
}

/// Submit an Azure Microsoft.Quota create_or_update for a compute family
/// against an injectable ARM client (Python
/// `client.quota.begin_create_or_update(...).result()` — an LRO wait).
pub async fn azure_request_increase_with_client(
    client: &ArmClient,
    location: &str,
    family_name: &str,
    new_limit: i64,
) -> Result<Value, AzureError> {
    let scope = azure_quota_scope(client.subscription(), location);
    let path = format!(
        "/{scope}/providers/Microsoft.Quota/quotas/{family_name}?api-version={AZURE_QUOTA_API_VERSION}"
    );
    let resp = client
        .put_lro(
            &path,
            &azure_quota_body(family_name, new_limit),
            &format!("quota create_or_update {family_name}"),
        )
        .await?;
    let name = resp
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or(family_name);
    Ok(json!({"name": name, "available": true}))
}

/// Submit an Azure Microsoft.Quota create_or_update for a compute family.
/// Python `_azure_request_increase`.
///
/// Returns {"available": True, "name": ...} on success or
/// {"available": False, "reason": ...} when AZURE_SUBSCRIPTION_ID is
/// empty; the latter surfaces as an informational result-list entry
/// instead of aborting a multi-provider fan-out.
pub async fn azure_request_increase(
    subscription: &str,
    location: &str,
    family_name: &str,
    new_limit: i64,
) -> Result<Value, AzureError> {
    if subscription.is_empty() {
        return Ok(json!({"available": false, "reason": "AZURE_SUBSCRIPTION_ID unset"}));
    }
    let client = ArmClient::new(subscription);
    azure_request_increase_with_client(&client, location, family_name, new_limit).await
}
