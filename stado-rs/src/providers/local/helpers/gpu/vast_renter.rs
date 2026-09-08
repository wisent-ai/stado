//! Whether a paying Vast.ai renter is already holding this machine.
//!
//! The agent asks before it claims: a rented box's GPU is sold, and claiming
//! on top of a renter burns their paid hour and ours at the same time.

/// Python `VAST_API`.
pub const VAST_API: &str = "https://console.vast.ai/api/v0";

/// Check if any Vast.ai instance is currently rented on this machine.
/// Python `_vast_has_renter`.
///
/// No exception swallow: a failed API call would otherwise be silently
/// treated as 'no renter' and the agent would claim jobs on top of a paid
/// Vast.ai renter, wasting both the renter's GPU time and ours. Caller
/// must crash visibly so the operator notices Vast.ai outage — hence the
/// `Err` propagation (and `error_for_status`, matching urllib's raise on
/// HTTP errors).
pub async fn vast_has_renter() -> anyhow::Result<bool> {
    let api_key = crate::skarbiec::read_string("stado-vast", "api_key")
        .await?
        .unwrap_or_default();
    if api_key.is_empty() {
        return Ok(false);
    }
    let body: serde_json::Value = reqwest::Client::new()
        .get(format!("{VAST_API}/instances?owner=me"))
        .bearer_auth(api_key)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    Ok(has_running_instance(&body))
}

/// Pure: any instance in the Vast.ai /instances payload with
/// `actual_status == "running"`.
pub fn has_running_instance(body: &serde_json::Value) -> bool {
    body.get("instances")
        .and_then(serde_json::Value::as_array)
        .is_some_and(|instances| {
            instances.iter().any(|i| {
                i.get("actual_status").and_then(serde_json::Value::as_str) == Some("running")
            })
        })
}
