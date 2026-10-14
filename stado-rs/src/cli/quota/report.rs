//! The report: what `stado quota request list` prints. One section per
//! provider — GCP's Cloud Quotas preferences bucketed by state, Azure's
//! open support tickets split by who is being waited on — or the whole
//! cross-provider payload as one JSON object under `--json`.

use serde_json::Value;

use super::common::{echo_json, parse_providers, quota_adapter};
use crate::cli::CmdError;
use crate::scheduler::dispatch::{quota_replies, quota_skus};

/// Python `quota_requests`: cross-provider in-flight requests + support
/// communications.
pub(super) async fn requests(
    providers_arg: &str,
    state_filter: &str,
    awaiting_customer: bool,
    as_json: bool,
) -> Result<(), CmdError> {
    let providers = parse_providers(providers_arg)?;
    // Insertion-ordered payload (Python dict in `providers` order); the
    // --json dump sorts keys anyway.
    let mut payload: Vec<(String, Vec<Value>)> = Vec::new();
    for provider in &providers {
        match quota_adapter(provider) {
            Some(crate::capabilities::QuotaAdapter::Gcp) => {
                let client = quota_skus::CloudQuotasClient::new(crate::config::project())
                    .await
                    .map_err(CmdError::from)?;
                let mut rows = quota_skus::gcp_request_status(&client)
                    .await
                    .map_err(CmdError::from)?;
                if !state_filter.is_empty() {
                    rows.retain(|r| r.get("state").and_then(Value::as_str) == Some(state_filter));
                }
                payload.push((provider.clone(), rows));
            }
            Some(crate::capabilities::QuotaAdapter::Azure) => {
                let mut rows =
                    quota_replies::list_open_azure_tickets(&quota_replies::SystemAzRunner)
                        .map_err(CmdError::from)?;
                if awaiting_customer {
                    rows.retain(|r| {
                        r.get("awaiting_customer").and_then(Value::as_bool) == Some(true)
                    });
                }
                payload.push((provider.clone(), rows));
            }
            _ => {}
        }
    }
    if as_json {
        let map: serde_json::Map<String, Value> = payload
            .into_iter()
            .map(|(provider, rows)| (provider, Value::Array(rows)))
            .collect();
        echo_json(&Value::Object(map));
        return Ok(());
    }
    for (provider, rows) in &payload {
        println!("\n=== {provider} ({} rows) ===", rows.len());
        if rows.is_empty() {
            println!("  (empty)");
            continue;
        }
        if quota_adapter(provider) == Some(crate::capabilities::QuotaAdapter::Gcp) {
            let mut buckets: std::collections::BTreeMap<String, usize> = Default::default();
            for r in rows {
                *buckets
                    .entry(match r.get("state").and_then(Value::as_str) {
                        Some(state) => state.to_string(),
                        None => "not reported".to_string(),
                    })
                    .or_insert(0) += 1;
            }
            let summary: Vec<String> = buckets
                .iter()
                .map(|(state, n)| format!("{state}={n}"))
                .collect();
            println!("  by state: {}", summary.join(", "));
            println!(
                "  {:<20} {:<20} {:<16} {:>5} {:>8}",
                "STATE", "FAMILY", "REGION", "PREF", "GRANTED"
            );
            let mut sorted: Vec<&Value> = rows.iter().collect();
            sorted.sort_by_key(|r| {
                (
                    r.get("state")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string(),
                    r.get("gpu_family")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string(),
                    r.get("region")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string(),
                )
            });
            for r in sorted {
                let state = match r.get("state").and_then(Value::as_str) {
                    Some(state) if !state.is_empty() => state,
                    _ => "not reported",
                };
                let family = match r.get("gpu_family").and_then(Value::as_str) {
                    Some(family) if !family.is_empty() => family,
                    _ => "(no gpu family)",
                };
                let region = match r.get("region").and_then(Value::as_str) {
                    Some(region) if !region.is_empty() => region,
                    _ => "(no region)",
                };
                // A request without a preferred value says so; it printed 0,
                // then a dash. A grant GCP has not made yet says not granted.
                let pref = match r.get("preferred_value").and_then(Value::as_i64) {
                    Some(pref) => pref.to_string(),
                    None => "not stated".to_string(),
                };
                let granted = match r.get("granted_value") {
                    Some(Value::Null) | None => "not granted".to_string(),
                    Some(v) => v.to_string(),
                };
                println!(
                    "  {:<20} {:<20} {:<16} {pref:>5} {granted:>8}",
                    state, family, region,
                );
            }
        } else if quota_adapter(provider) == Some(crate::capabilities::QuotaAdapter::Azure) {
            let ms_n = rows
                .iter()
                .filter(|r| r.get("awaiting_customer").and_then(Value::as_bool) == Some(true))
                .count();
            println!(
                "  awaiting customer: {ms_n}    awaiting Microsoft: {}",
                rows.len() - ms_n
            );
            println!(
                "  {:<22} {:<11} {:<22} LAST_BODY_SNIPPET",
                "REGION", "AWAIT_CUST", "LAST_SENT"
            );
            let mut sorted: Vec<&Value> = rows.iter().collect();
            sorted.sort_by_key(|r| {
                (
                    r.get("awaiting_customer").and_then(Value::as_bool) != Some(true),
                    r.get("region")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string(),
                )
            });
            for r in sorted {
                let awaiting = if r.get("awaiting_customer").and_then(Value::as_bool) == Some(true)
                {
                    "Y"
                } else {
                    "N"
                };
                let region = match r.get("region").and_then(Value::as_str) {
                    Some(region) if !region.is_empty() => region,
                    _ => "(no region in title)",
                };
                let sent = match r.get("last_sent").and_then(Value::as_str) {
                    Some(sent) if !sent.is_empty() => sent,
                    _ => "(no message)",
                };
                let snippet = match r.get("last_body_snippet").and_then(Value::as_str) {
                    Some(snippet) => snippet,
                    None => "(no message body)",
                };
                println!("  {:<22} {awaiting:<11} {:<22} {}", region, sent, snippet);
            }
        }
    }
    Ok(())
}
