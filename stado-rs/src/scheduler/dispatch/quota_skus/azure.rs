//! Azure arm: the Compute SKU-table filter, the ARM catalog enumeration,
//! and the Microsoft.Quota create_or_update fan-out across every family
//! the subscription advertises.

use std::num::NonZeroU64;

use serde_json::{json, Value};

/// The capability the Compute Resource SKUs API gives a VM size whose GPU
/// count is declared (`capabilities[].name`, value a count as text).
const GPU_CAPABILITY: &str = "GPUs";

/// The GPUs a SKU declares in its own capabilities, when it declares any.
fn declared_gpus(sku: &Value) -> Option<NonZeroU64> {
    sku.get("capabilities")
        .and_then(Value::as_array)?
        .iter()
        .find(|capability| capability.get("name").and_then(Value::as_str) == Some(GPU_CAPABILITY))
        .and_then(|capability| capability.get("value").and_then(Value::as_str))
        .and_then(|count| count.parse::<NonZeroU64>().ok())
}

/// The pure SKU-table half of the catalog: every SKU that declares at least
/// one GPU in its capabilities, one row per (family, location). A family
/// name containing NC, ND, NV or GPU decided this before; Azure names its
/// families as it pleases and states the GPU count on each SKU.
pub fn azure_rows_from_skus(skus: &[Value]) -> Vec<Value> {
    let mut out = Vec::new();
    for sku in skus {
        if declared_gpus(sku).is_none() {
            continue;
        }
        let (Some(family), Some(name)) = (
            sku.get("family").and_then(Value::as_str),
            sku.get("name").and_then(Value::as_str),
        ) else {
            continue;
        };
        // Mark each (family, location) row once; the SKU table has many
        // SKUs per family, so we dedupe at print/aggregate time.
        if let Some(locations) = sku.get("locations").and_then(Value::as_array) {
            for loc in locations.iter().filter_map(Value::as_str) {
                out.push(json!({
                    "provider": crate::capabilities::ProviderId::Azure.as_str(),
                    "family": family,
                    "sku": name,
                    "location": loc,
                }));
            }
        }
    }
    out
}

/// Enumerate Azure Compute GPU VM families across every location available to
/// the subscription through ARM. Authentication uses managed identity or the
/// Skarbiec `cloud-azure` role; Azure CLI is not consulted.
pub async fn azure_catalog() -> Vec<Value> {
    let subscription = crate::config::azure_subscription_id();
    if subscription.is_empty() {
        return vec![json!({
            "provider": crate::capabilities::ProviderId::Azure.as_str(),
            "ok": false,
            "error": "AZURE_SUBSCRIPTION_ID is required",
        })];
    }
    let http = reqwest::Client::new();
    let token = match crate::remote::azure_token::identity_bearer_token(
        &http,
        "https://management.azure.com/.default",
        "https://management.azure.com",
    )
    .await
    {
        Ok(token) => token,
        Err(err) => {
            return vec![json!({
                "provider": crate::capabilities::ProviderId::Azure.as_str(),
                "ok": false,
                "error": err.to_string(),
            })];
        }
    };
    let mut next = Some(format!(
        "https://management.azure.com/subscriptions/{subscription}/providers/Microsoft.Compute/skus?api-version=2021-07-01"
    ));
    let mut skus = Vec::new();
    while let Some(url) = next.take() {
        let response = match crate::wait::request(http.get(url).bearer_auth(&token)).await {
            Ok(response) => response,
            Err(err) => {
                return vec![json!({
                    "provider": crate::capabilities::ProviderId::Azure.as_str(),
                    "ok": false,
                    "error": err.to_string(),
                })];
            }
        };
        let status = response.status();
        let body: Value = match response.json().await {
            Ok(body) => body,
            Err(err) => {
                return vec![json!({
                    "provider": crate::capabilities::ProviderId::Azure.as_str(),
                    "ok": false,
                    "error": err.to_string(),
                })];
            }
        };
        if !status.is_success() {
            return vec![json!({
                "provider": crate::capabilities::ProviderId::Azure.as_str(),
                "ok": false,
                "error": format!("Azure Compute SKU list returned HTTP {status}: {body}"),
            })];
        }
        skus.extend(
            body.get("value")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .cloned(),
        );
        next = body
            .get("nextLink")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .map(str::to_string);
    }
    azure_rows_from_skus(&skus)
}

/// Fan out Microsoft.Quota create_or_update for every distinct GPU family
/// the subscription advertises × every location the subscription serves
/// any GPU SKU in (Python `azure_request_all_families`). Same
/// default-union-of-all-locations pattern as GCP: per-family location
/// lists in az vm list-skus are conservative (only locations the
/// subscription has access to for that exact family), but request-all
/// defaults to the global union so the subscription builds quota
/// everywhere any family is available. Per-target "family not in this
/// location" failures are captured in the result list, not raised.
pub async fn azure_request_all_families(new_limit: i64, locations: &[String]) -> Vec<Value> {
    let catalog = azure_catalog().await;
    let mut families: std::collections::BTreeSet<String> = Default::default();
    let mut all_locs: std::collections::BTreeSet<String> = Default::default();
    for row in &catalog {
        if let Some(fam) = row.get("family").and_then(Value::as_str) {
            families.insert(fam.to_string());
        }
        if let Some(loc) = row.get("location").and_then(Value::as_str) {
            all_locs.insert(loc.to_string());
        }
    }
    let requested: std::collections::BTreeSet<&str> =
        locations.iter().map(String::as_str).collect();
    let target_locs: Vec<&String> = if requested.is_empty() {
        all_locs.iter().collect()
    } else {
        all_locs
            .iter()
            .filter(|l| requested.contains(l.as_str()))
            .collect()
    };
    let subscription = crate::config::azure_subscription_id();
    let mut out = Vec::new();
    for loc in target_locs {
        for fam in &families {
            let row = match super::quota_request::azure_request_increase(
                subscription,
                loc,
                fam,
                new_limit,
            )
            .await
            {
                Ok(r) if r.get("available").and_then(Value::as_bool) == Some(false) => json!({
                    "provider": crate::capabilities::ProviderId::Azure.as_str(), "location": loc, "family": fam, "ok": false,
                    "error": "Microsoft.Quota reports this family unavailable here",
                    "reason": r.get("reason"),
                }),
                Ok(r) => {
                    let mut row = json!({
                        "provider": crate::capabilities::ProviderId::Azure.as_str(), "location": loc, "family": fam, "ok": true,
                    });
                    super::quota_request::merge_object(&mut row, r);
                    row
                }
                Err(err) => json!({
                    "provider": crate::capabilities::ProviderId::Azure.as_str(), "location": loc, "family": fam, "ok": false,
                    "error": format!("AzureError: {err}"),
                }),
            };
            out.push(row);
        }
    }
    out
}
