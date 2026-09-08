//! Request submission: `request` asks for one accelerator's limit per
//! (provider, region), `request-all` asks for every GPU family the
//! catalog reports per region. Both refuse to run against GCP without a
//! contact email, because the Cloud Quotas API puts one on every
//! preference, and both print one row per submission.

use serde_json::Value;

use crate::cli::quota::common::{
    contact_email, echo_json, gcp_project_env, parse_providers, parse_regions, quota_adapter, take,
};
use crate::cli::CmdError;
use crate::scheduler::dispatch::{quota_request, quota_skus};

/// Python `quota_request`: one quota-increase request per (provider,
/// region) for ACCEL.
#[allow(clippy::too_many_arguments)]
pub(in crate::cli::quota) async fn request(
    accel: &str,
    new_limit: i64,
    regions_arg: &str,
    providers_arg: &str,
    justification: &str,
    email_arg: &str,
    as_json: bool,
) -> Result<(), CmdError> {
    let providers = parse_providers(providers_arg)?;
    let email = contact_email(email_arg);
    if email.is_empty()
        && providers
            .iter()
            .any(|provider| quota_adapter(provider) == Some(crate::capabilities::QuotaAdapter::Gcp))
    {
        return Err(CmdError::click(
            "--email is required for GCP (or set WC_QUOTA_CONTACT_EMAIL); the Cloud Quotas API requires a contact email on every preference.",
        ));
    }
    let regions = parse_regions(regions_arg);
    let results = quota_request::request_quota_increases(
        None,
        accel,
        new_limit,
        &providers,
        regions.as_deref(),
        justification,
        &email,
    )
    .await;
    if as_json {
        echo_json(&serde_json::to_value(&results)?);
        return Ok(());
    }
    println!("{:<8} {:<18} {:<3} DETAIL", "PROVIDER", "REGION/LOC", "OK");
    println!("{}", "-".repeat(80));
    let mut ok_count = 0;
    for r in &results {
        let rkey = r
            .get("region")
            .or_else(|| r.get("location"))
            .and_then(Value::as_str)
            .unwrap_or("-");
        let ok = r.get("ok").and_then(Value::as_bool).unwrap_or(false);
        if ok {
            ok_count += 1;
        }
        let detail = if ok {
            r.get("name")
                .and_then(Value::as_str)
                .unwrap_or("?")
                .to_string()
        } else {
            r.get("error")
                .and_then(Value::as_str)
                .unwrap_or("?")
                .to_string()
        };
        let provider = r.get("provider").and_then(Value::as_str).unwrap_or("?");
        println!(
            "{provider:<8} {rkey:<18} {:<3} {detail}",
            if ok { "Y" } else { "N" }
        );
    }
    println!("\n{ok_count}/{} succeeded", results.len());
    Ok(())
}

/// Python `quota_request_all`: one request per GPU family x region.
pub(in crate::cli::quota) async fn request_all(
    new_limit: i64,
    providers_arg: &str,
    regions_arg: &str,
    justification: &str,
    email_arg: &str,
    as_json: bool,
) -> Result<(), CmdError> {
    let providers = parse_providers(providers_arg)?;
    let explicit_regions = parse_regions(regions_arg).unwrap_or_default();
    let email = contact_email(email_arg);
    if email.is_empty()
        && providers
            .iter()
            .any(|provider| quota_adapter(provider) == Some(crate::capabilities::QuotaAdapter::Gcp))
    {
        return Err(CmdError::click(
            "--email is required for GCP (or set WC_QUOTA_CONTACT_EMAIL); \
             the Cloud Quotas API mandates a contact email on every preference.",
        ));
    }
    let mut results: Vec<Value> = Vec::new();
    for provider in &providers {
        match quota_adapter(provider) {
            Some(crate::capabilities::QuotaAdapter::Gcp) => {
                // Default = no region filter = every applicable_region the
                // catalog reports per family. The bulk submitter intersects
                // against this only if explicit_regions is non-empty. Don't
                // default to config::regions() — that's the dispatcher's
                // current dispatch list, not a quota policy.
                let client = quota_skus::CloudQuotasClient::new(&gcp_project_env())
                    .await
                    .map_err(|err| CmdError::click(err.to_string()))?;
                results.extend(
                    quota_skus::gcp_request_all_families(
                        &client,
                        new_limit,
                        &explicit_regions,
                        &email,
                        justification,
                    )
                    .await
                    .map_err(|err| CmdError::click(err.to_string()))?,
                );
            }
            Some(crate::capabilities::QuotaAdapter::Azure) => {
                results.extend(
                    quota_skus::azure_request_all_families(new_limit, &explicit_regions).await,
                );
            }
            _ => results.push(serde_json::json!({
                "provider": provider,
                "ok": false,
                "error": "no request-all impl for this provider",
            })),
        }
    }
    if as_json {
        echo_json(&serde_json::to_value(&results)?);
        return Ok(());
    }
    println!(
        "{:<8} {:<18} {:<22} {:<3} DETAIL",
        "PROVIDER", "REGION/LOC", "FAMILY", "OK"
    );
    println!("{}", "-".repeat(100));
    let mut ok_count = 0;
    for r in &results {
        let rkey = r
            .get("region")
            .or_else(|| r.get("location"))
            .and_then(Value::as_str)
            .unwrap_or("-");
        let fam = r
            .get("gpu_family")
            .or_else(|| r.get("family"))
            .and_then(Value::as_str)
            .unwrap_or("-");
        let ok = r.get("ok").and_then(Value::as_bool).unwrap_or(false);
        if ok {
            ok_count += 1;
        }
        let detail = if ok {
            r.get("name")
                .and_then(Value::as_str)
                .unwrap_or("?")
                .to_string()
        } else {
            r.get("error")
                .and_then(Value::as_str)
                .unwrap_or("?")
                .to_string()
        };
        let provider = r.get("provider").and_then(Value::as_str).unwrap_or("?");
        println!(
            "{provider:<8} {:<18} {:<22} {:<3} {:.60}",
            take(rkey, 16),
            take(fam, 20),
            if ok { "Y" } else { "N" },
            detail
        );
    }
    println!("\n{ok_count}/{} requests submitted", results.len());
    Ok(())
}
