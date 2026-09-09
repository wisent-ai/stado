//! The per-provider fan-outs and the cross-provider entrypoint, plus the
//! row merge every result entry is assembled with.

use serde_json::{json, Value};

use crate::catalog::AZURE_QUOTA_FAMILY_TO_ACCEL;
use crate::config;

use super::azure::azure_request_increase;
use super::error::py_type_name;
use super::gcp::gcp_request_increase;
use super::quota_skus::{gcp_project_env, CloudQuotasClient};

/// Python `_gcp_fanout`. Per-target failures are captured as result rows
/// rather than aborting the rest of the fan-out. `gcp_client` is
/// injectable for tests; `None` resolves a live client, and a
/// client-construction (auth) failure is reported per region exactly like
/// Python constructing the SDK client inside the per-region try.
pub async fn gcp_fanout(
    gcp_client: Option<&CloudQuotasClient>,
    accel: &str,
    new_limit: i64,
    regions: Option<&[String]>,
    justification: &str,
    contact_email: &str,
) -> Vec<Value> {
    let targets: Vec<String> = regions
        .map(<[String]>::to_vec)
        .unwrap_or_else(|| config::regions().to_vec());
    let owned;
    let client = match gcp_client {
        Some(client) => Some(client),
        None => match CloudQuotasClient::new(&gcp_project_env()).await {
            Ok(c) => {
                owned = c;
                Some(&owned)
            }
            Err(_) => None,
        },
    };
    let mut out = Vec::new();
    for region in &targets {
        let row = match client {
            Some(client) => {
                match gcp_request_increase(
                    client,
                    region,
                    accel,
                    new_limit,
                    justification,
                    contact_email,
                )
                .await
                {
                    Ok(r) => {
                        let mut row = json!({"provider": crate::capabilities::ProviderId::Gcp.as_str(), "region": region, "ok": true});
                        merge_object(&mut row, r);
                        row
                    }
                    Err(err) => json!({
                    "provider": crate::capabilities::ProviderId::Gcp.as_str(), "region": region, "ok": false,
                        "error": format!("{}: {err}", py_type_name(&err)),
                    }),
                }
            }
            None => json!({
                "provider": crate::capabilities::ProviderId::Gcp.as_str(), "region": region, "ok": false,
                "error": "DefaultCredentialsError: Cloud Quotas client construction failed",
            }),
        };
        out.push(row);
    }
    out
}

/// Python `_azure_fanout`.
pub async fn azure_fanout(accel: &str, new_limit: i64, regions: Option<&[String]>) -> Vec<Value> {
    let mut families: Vec<&str> = AZURE_QUOTA_FAMILY_TO_ACCEL
        .iter()
        .filter(|(_, a)| **a == accel)
        .map(|(f, _)| *f)
        .collect();
    // Deviation: Python iterates the literal dict in insertion order; the
    // Rust catalog table is a HashMap, so rows are sorted for determinism.
    families.sort_unstable();
    if families.is_empty() {
        return vec![json!({
            "provider": crate::capabilities::ProviderId::Azure.as_str(), "ok": false,
            "error": format!("no Azure compute family matches accel '{accel}'"),
        })];
    }
    let targets: Vec<String> = regions
        .map(<[String]>::to_vec)
        .unwrap_or_else(|| config::azure_locations().to_vec());
    let subscription = config::azure_subscription_id();
    let mut out = Vec::new();
    for loc in &targets {
        for fam in &families {
            let row = match azure_request_increase(subscription, loc, fam, new_limit).await {
                Ok(r) if r.get("available").and_then(Value::as_bool) == Some(false) => json!({
                    "provider": crate::capabilities::ProviderId::Azure.as_str(), "location": loc, "family": fam, "ok": false,
                    "error": r.get("reason").and_then(Value::as_str).unwrap_or("not available"),
                }),
                Ok(r) => {
                    let mut row = json!({"provider": crate::capabilities::ProviderId::Azure.as_str(), "location": loc, "family": fam, "ok": true});
                    merge_object(&mut row, r);
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

/// Fan out quota-increase requests across providers and regions. Python
/// `request_quota_increases`.
///
/// For each provider in `providers`, iterate `regions` (or the provider's
/// configured region/location list when None) and submit one
/// quota-increase request per (provider, region). Per-target failures are
/// captured in the result list rather than aborting the rest of the
/// fan-out: each entry carries `provider`, a region/location key, `ok`
/// (bool), and either `name` (success) or `error`.
pub async fn request_quota_increases(
    gcp_client: Option<&CloudQuotasClient>,
    accel: &str,
    new_limit: i64,
    providers: &[String],
    regions: Option<&[String]>,
    justification: &str,
    contact_email: &str,
) -> Vec<Value> {
    let mut out = Vec::new();
    for provider in providers {
        let adapter =
            crate::capabilities::variant(crate::capabilities::RuntimeFacet::Quota, provider)
                .map(|variant| variant.adapter);
        match adapter {
            Some(crate::capabilities::RuntimeAdapter::Quota(
                crate::capabilities::QuotaAdapter::Gcp,
            )) => out.extend(
                gcp_fanout(
                    gcp_client,
                    accel,
                    new_limit,
                    regions,
                    justification,
                    contact_email,
                )
                .await,
            ),
            Some(crate::capabilities::RuntimeAdapter::Quota(
                crate::capabilities::QuotaAdapter::Azure,
            )) => out.extend(azure_fanout(accel, new_limit, regions).await),
            _ => out.push(json!({
                "provider": provider, "ok": false,
                "error": "no quota-increase impl for this provider",
            })),
        }
    }
    out
}

/// Python `{**base, **r}` row merge.
pub(crate) fn merge_object(row: &mut Value, extra: Value) {
    if let (Value::Object(base), Value::Object(extra)) = (row, extra) {
        base.extend(extra);
    }
}
