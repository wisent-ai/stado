//! Request submission: `request create ACCEL` asks for one accelerator's limit
//! per (provider, region), `request create --every-family` asks for every GPU
//! family the catalog reports per region. Both refuse to run against GCP without a
//! contact email, because the Cloud Quotas API puts one on every
//! preference, and both print one row per submission.

use serde_json::Value;

use crate::cli::quota::common::{
    contact_email, echo_json, parse_providers, parse_regions, quota_adapter,
};
use crate::cli::reporting::field;
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
    let email = contact_email(email_arg)?;
    if email.is_empty()
        && providers
            .iter()
            .any(|provider| quota_adapter(provider) == Some(crate::capabilities::QuotaAdapter::Gcp))
    {
        return Err(CmdError::refused(
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
    let header = format!("{:<8} {:<18} {:<3} DETAIL", "PROVIDER", "REGION/LOC", "OK");
    println!("{header}");
    println!("{}", "-".repeat(header.len()));
    for r in &results {
        let rkey = match r
            .get("region")
            .or_else(|| r.get("location"))
            .and_then(Value::as_str)
        {
            Some(rkey) => rkey,
            None => "(no region)",
        };
        let ok = succeeded(r);
        let detail = if ok {
            field(&r["name"])
        } else {
            field(&r["error"])
        };
        let provider = field(&r["provider"]);
        println!(
            "{provider:<8} {rkey:<18} {:<3} {detail}",
            if ok { "Y" } else { "N" }
        );
    }
    let ok_count = results.iter().filter(|r| succeeded(r)).count();
    println!("\n{ok_count}/{} succeeded", results.len());
    Ok(())
}

/// Whether a result row states that its request succeeded; a row that does
/// not say so did not.
fn succeeded(row: &Value) -> bool {
    row.get("ok").and_then(Value::as_bool) == Some(true)
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
    // No --regions names no region explicitly; the plan then reads them.
    let explicit_regions = match parse_regions(regions_arg) {
        Some(regions) => regions,
        None => Vec::new(),
    };
    let email = contact_email(email_arg)?;
    if email.is_empty()
        && providers
            .iter()
            .any(|provider| quota_adapter(provider) == Some(crate::capabilities::QuotaAdapter::Gcp))
    {
        return Err(CmdError::refused(
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
                let client = quota_skus::CloudQuotasClient::new(crate::config::project())
                    .await
                    .map_err(CmdError::from)?;
                results.extend(
                    quota_skus::gcp_request_all_families(
                        &client,
                        new_limit,
                        &explicit_regions,
                        &email,
                        justification,
                    )
                    .await
                    .map_err(CmdError::from)?,
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
                "error": "no every-family request implementation for this provider",
            })),
        }
    }
    if as_json {
        echo_json(&serde_json::to_value(&results)?);
        return Ok(());
    }
    let header = format!(
        "{:<8} {:<18} {:<22} {:<3} DETAIL",
        "PROVIDER", "REGION/LOC", "FAMILY", "OK"
    );
    println!("{header}");
    println!("{}", "-".repeat(header.len()));
    for r in &results {
        let rkey = match r
            .get("region")
            .or_else(|| r.get("location"))
            .and_then(Value::as_str)
        {
            Some(rkey) => rkey,
            None => "(no region)",
        };
        let fam = match r
            .get("gpu_family")
            .or_else(|| r.get("family"))
            .and_then(Value::as_str)
        {
            Some(fam) => fam,
            None => "(no family)",
        };
        let ok = succeeded(r);
        let detail = if ok {
            field(&r["name"])
        } else {
            field(&r["error"])
        };
        let provider = field(&r["provider"]);
        println!(
            "{provider:<8} {:<18} {:<22} {:<3} {}",
            rkey,
            fam,
            if ok { "Y" } else { "N" },
            detail
        );
    }
    let ok_count = results.iter().filter(|r| succeeded(r)).count();
    println!("\n{ok_count}/{} requests submitted", results.len());
    Ok(())
}
