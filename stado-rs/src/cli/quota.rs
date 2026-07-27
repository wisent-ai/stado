//! `stado quota` command group — READ side (`show`, `catalog`) and WRITE
//! side (`request`, `request-all`, `requests`, `azure-replies`,
//! `azure-escalate`).
//!
//! Port of the `quota` group in `stado/cli.py`.

use serde_json::Value;

use super::{CmdError, QuotaCommands};
use crate::queue::JobStorage;
use crate::scheduler::dispatch::{quota_replies, quota_request, quota_skus};
use crate::scheduler::quota;

/// Dispatch one `quota` subcommand; `None` is the bare `quota` group,
/// which Python redirects to `quota show` with the group-level --json.
#[allow(clippy::too_many_lines)]
pub(super) async fn dispatch(json: bool, sub: &Option<QuotaCommands>) -> Result<(), CmdError> {
    match sub {
        None => show(json).await,
        Some(QuotaCommands::Show { json: sub_json }) => show(json || *sub_json).await,
        Some(QuotaCommands::Catalog {
            provider,
            json: sub_json,
        }) => catalog(provider, *sub_json).await,
        Some(QuotaCommands::Request {
            accel,
            new_limit,
            region,
            provider,
            justification,
            email,
            json: sub_json,
        }) => {
            request(
                accel,
                *new_limit,
                region,
                provider,
                justification,
                email,
                *sub_json,
            )
            .await
        }
        Some(QuotaCommands::RequestAll {
            new_limit,
            provider,
            region,
            justification,
            email,
            json: sub_json,
        }) => {
            request_all(
                *new_limit,
                provider,
                region,
                justification,
                email,
                *sub_json,
            )
            .await
        }
        Some(QuotaCommands::Requests {
            provider,
            state,
            awaiting_customer,
            json: sub_json,
        }) => requests(provider, state, *awaiting_customer, *sub_json).await,
        Some(QuotaCommands::AzureReplies { dry_run, email }) => {
            azure_replies(*dry_run, email).await
        }
        Some(QuotaCommands::AzureEscalate { dry_run, email }) => {
            azure_escalate(*dry_run, email).await
        }
    }
}

/// Python `click.echo(json.dumps(payload, indent=2, sort_keys=True))`.
fn echo_json(value: &Value) {
    let pretty = serde_json::to_string_pretty(value).expect("Value serialization is infallible");
    println!("{}", crate::models::ensure_ascii(&pretty));
}

/// Python `str[:n]` truncation.
fn take(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

/// Python `quota_show`: table of live cloud quota minus reservation minus
/// running per provider (or --json).
async fn show(as_json: bool) -> Result<(), CmdError> {
    let store = JobStorage::new().await?;
    let summary = quota::summarize_quotas(&store)
        .await
        .map_err(|err| CmdError::click(err.to_string()))?;
    if as_json {
        echo_json(&serde_json::to_value(&summary)?);
        return Ok(());
    }
    println!(
        "{:<10} {:<22} {:>6} {:>9} {:>5} {:>6}",
        "PROVIDER", "ACCEL", "TOTAL", "RESERVED", "USED", "AVAIL"
    );
    println!("{}", "-".repeat(70));
    let mut grand_total: std::collections::BTreeMap<String, i64> = Default::default();
    let mut grand_avail: std::collections::BTreeMap<String, i64> = Default::default();
    for (provider_name, rows) in &summary {
        if rows.is_empty() {
            println!(
                "{provider_name:<10} (no quota visible — credentials missing or SDK not installed)"
            );
            continue;
        }
        for (accel, row) in rows {
            println!(
                "{provider_name:<10} {accel:<22} {:>6} {:>9} {:>5} {:>6}",
                row.total, row.reserved, row.used, row.available
            );
            *grand_total.entry(accel.clone()).or_insert(0) += row.total;
            *grand_avail.entry(accel.clone()).or_insert(0) += row.available;
        }
    }
    if summary.len() > 1 && !grand_total.is_empty() {
        println!("{}", "-".repeat(70));
        for (accel, total) in &grand_total {
            let avail = grand_avail.get(accel).copied().unwrap_or(0);
            println!(
                "{:<10} {accel:<22} {total:>6} {:>9} {:>5} {avail:>6}",
                "TOTAL", "", ""
            );
        }
    }
    Ok(())
}

/// Python `quota_catalog`: full GPU catalog per provider (or --json).
async fn catalog(providers_arg: &str, as_json: bool) -> Result<(), CmdError> {
    let providers: Vec<String> = providers_arg
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect();
    let providers = if providers.is_empty() {
        crate::config::wc_providers().to_vec()
    } else {
        providers
    };
    let cats = quota_skus::all_catalogs(&providers, None)
        .await
        .map_err(|err| CmdError::click(err.to_string()))?;
    if as_json {
        echo_json(&serde_json::to_value(&cats)?);
        return Ok(());
    }
    for (provider, rows) in &cats {
        println!("\n=== {provider} ({} rows) ===", rows.len());
        if rows.is_empty() {
            println!("  (empty)");
            continue;
        }
        if rows
            .iter()
            .any(|r| r.get("ok") == Some(&Value::Bool(false)))
        {
            for row in rows {
                if row.get("ok") == Some(&Value::Bool(false)) {
                    let error = row.get("error").and_then(Value::as_str).unwrap_or("?");
                    println!("  ERROR: {error}");
                }
            }
            continue;
        }
        if provider == "gcp" {
            println!(
                "  {:<52} {:<20} {:<16} {:>6}",
                "QUOTA_ID", "FAMILY", "REGION", "LIMIT"
            );
            let mut sorted: Vec<&Value> = rows.iter().collect();
            sorted.sort_by_key(|r| {
                (
                    r.get("quota_id")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string(),
                    r.get("region")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string(),
                )
            });
            for row in sorted {
                let quota_id = row.get("quota_id").and_then(Value::as_str).unwrap_or("?");
                let quota_id = if quota_id.is_empty() { "?" } else { quota_id };
                let family = row.get("gpu_family").and_then(Value::as_str).unwrap_or("-");
                let family = if family.is_empty() { "-" } else { family };
                let region = row.get("region").and_then(Value::as_str).unwrap_or("-");
                let region = if region.is_empty() { "-" } else { region };
                let limit = match row.get("limit") {
                    Some(Value::Number(n)) => n.to_string(),
                    _ => "-".to_string(),
                };
                println!(
                    "  {:<52} {:<20} {:<16} {:>6}",
                    take(quota_id, 50),
                    take(family, 18),
                    take(region, 14),
                    limit
                );
            }
        } else if provider == "azure" {
            let mut seen_fam: std::collections::BTreeMap<
                String,
                std::collections::BTreeSet<String>,
            > = Default::default();
            for row in rows {
                let family = row.get("family").and_then(Value::as_str).unwrap_or("");
                let location = row.get("location").and_then(Value::as_str).unwrap_or("");
                seen_fam
                    .entry(family.to_string())
                    .or_default()
                    .insert(location.to_string());
            }
            println!("  {:<36} LOCATIONS", "FAMILY");
            for (family, locations) in &seen_fam {
                let locs: Vec<&String> = locations.iter().collect();
                let head: Vec<&str> = locs.iter().take(5).map(|s| s.as_str()).collect();
                let more = if locs.len() > 5 { ", …" } else { "" };
                println!(
                    "  {:<36} {} ({}{})",
                    take(family, 34),
                    locs.len(),
                    head.join(", "),
                    more
                );
            }
        }
    }
    Ok(())
}

// ---- write-side subcommands ----

/// Python's CSV-flag parse (`[p.strip() for p in arg.split(",") if
/// p.strip()] or WC_PROVIDERS`).
fn parse_providers(arg: &str) -> Vec<String> {
    let parsed: Vec<String> = arg
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect();
    if parsed.is_empty() {
        crate::config::wc_providers().to_vec()
    } else {
        parsed
    }
}

/// Python's regions CSV parse (`... or None`).
fn parse_regions(arg: &str) -> Option<Vec<String>> {
    let parsed: Vec<String> = arg
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect();
    if parsed.is_empty() {
        None
    } else {
        Some(parsed)
    }
}

/// `--email` or $WC_QUOTA_CONTACT_EMAIL; "" when neither is set.
fn contact_email(flag: &str) -> String {
    if !flag.is_empty() {
        return flag.to_string();
    }
    std::env::var("WC_QUOTA_CONTACT_EMAIL")
        .unwrap_or_default()
        .trim()
        .to_string()
}

/// Python `quota_request`: one quota-increase request per (provider,
/// region) for ACCEL.
#[allow(clippy::too_many_arguments)]
async fn request(
    accel: &str,
    new_limit: i64,
    regions_arg: &str,
    providers_arg: &str,
    justification: &str,
    email_arg: &str,
    as_json: bool,
) -> Result<(), CmdError> {
    let email = contact_email(email_arg);
    if email.is_empty() {
        return Err(CmdError::click(
            "--email is required (or set WC_QUOTA_CONTACT_EMAIL); the GCP \
             Cloud Quotas API requires a contact email on every preference.",
        ));
    }
    let providers = parse_providers(providers_arg);
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
async fn request_all(
    new_limit: i64,
    providers_arg: &str,
    regions_arg: &str,
    justification: &str,
    email_arg: &str,
    as_json: bool,
) -> Result<(), CmdError> {
    let providers = parse_providers(providers_arg);
    let explicit_regions = parse_regions(regions_arg).unwrap_or_default();
    let email = contact_email(email_arg);
    if email.is_empty() && providers.iter().any(|p| p == "gcp") {
        return Err(CmdError::click(
            "--email is required for GCP (or set WC_QUOTA_CONTACT_EMAIL); \
             the Cloud Quotas API mandates a contact email on every preference.",
        ));
    }
    let mut results: Vec<Value> = Vec::new();
    for provider in &providers {
        match provider.as_str() {
            "gcp" => {
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
            "azure" => {
                results.extend(
                    quota_skus::azure_request_all_families(new_limit, &explicit_regions).await,
                );
            }
            other => results.push(serde_json::json!({
                "provider": other,
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

/// The GCP project the write side targets (env-only, like the Python).
fn gcp_project_env() -> String {
    std::env::var("GCP_PROJECT").unwrap_or_else(|_| "wisent-480400".to_string())
}

/// Python `quota_requests`: cross-provider in-flight requests + support
/// communications.
async fn requests(
    providers_arg: &str,
    state_filter: &str,
    awaiting_customer: bool,
    as_json: bool,
) -> Result<(), CmdError> {
    let providers = parse_providers(providers_arg);
    // Insertion-ordered payload (Python dict in `providers` order); the
    // --json dump sorts keys anyway.
    let mut payload: Vec<(String, Vec<Value>)> = Vec::new();
    for provider in &providers {
        match provider.as_str() {
            "gcp" => {
                let client = quota_skus::CloudQuotasClient::new(&gcp_project_env())
                    .await
                    .map_err(|err| CmdError::click(err.to_string()))?;
                let mut rows = quota_skus::gcp_request_status(&client)
                    .await
                    .map_err(|err| CmdError::click(err.to_string()))?;
                if !state_filter.is_empty() {
                    rows.retain(|r| r.get("state").and_then(Value::as_str) == Some(state_filter));
                }
                payload.push(("gcp".to_string(), rows));
            }
            "azure" => {
                let mut rows =
                    quota_replies::list_open_azure_tickets(&quota_replies::SystemAzRunner)
                        .map_err(|err| CmdError::click(err.to_string()))?;
                if awaiting_customer {
                    rows.retain(|r| {
                        r.get("awaiting_customer").and_then(Value::as_bool) == Some(true)
                    });
                }
                payload.push(("azure".to_string(), rows));
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
        if provider == "gcp" {
            let mut buckets: std::collections::BTreeMap<String, usize> = Default::default();
            for r in rows {
                *buckets
                    .entry(
                        r.get("state")
                            .and_then(Value::as_str)
                            .unwrap_or("?")
                            .to_string(),
                    )
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
                let state = r.get("state").and_then(Value::as_str).unwrap_or("?");
                let state = if state.is_empty() { "?" } else { state };
                let family = r.get("gpu_family").and_then(Value::as_str).unwrap_or("-");
                let family = if family.is_empty() { "-" } else { family };
                let region = r.get("region").and_then(Value::as_str).unwrap_or("-");
                let region = if region.is_empty() { "-" } else { region };
                let pref = r
                    .get("preferred_value")
                    .and_then(Value::as_i64)
                    .unwrap_or(0);
                let granted = match r.get("granted_value") {
                    Some(Value::Null) | None => "-".to_string(),
                    Some(v) => v.to_string(),
                };
                println!(
                    "  {:<20} {:<20} {:<16} {pref:>5} {granted:>8}",
                    take(state, 18),
                    take(family, 18),
                    take(region, 14),
                );
            }
        } else if provider == "azure" {
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
                let region = r.get("region").and_then(Value::as_str).unwrap_or("?");
                let region = if region.is_empty() { "?" } else { region };
                let sent = r.get("last_sent").and_then(Value::as_str).unwrap_or("-");
                let sent = if sent.is_empty() { "-" } else { sent };
                let snippet = r
                    .get("last_body_snippet")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                println!(
                    "  {:<22} {awaiting:<11} {:<22} {:.60}",
                    take(region, 20),
                    take(sent, 20),
                    snippet
                );
            }
        }
    }
    Ok(())
}

/// The Python CLI's Forbidden/permission detection on az failures.
fn support_permission_error(err: &quota_replies::RepliesError) -> Option<CmdError> {
    let stderr = err.stderr().trim();
    if stderr.contains("Forbidden") && stderr.contains("permission") {
        return Some(CmdError::click(
            "Microsoft.Support API returned Forbidden for the current \
             Azure credential. Owner on the subscription is NOT \
             sufficient — assign 'Support Request Contributor' on \
             subscription 9ae7cfa4-… to the user or service principal \
             running this command, then retry.",
        ));
    }
    None
}

/// Python `quota_azure_replies`: respond to Open Azure quota tickets
/// awaiting customer info.
async fn azure_replies(dry_run: bool, email_arg: &str) -> Result<(), CmdError> {
    let email = contact_email(email_arg);
    if email.is_empty() {
        return Err(CmdError::click(
            "--email is required (or set WC_QUOTA_CONTACT_EMAIL); the \
             reply body signs off with the customer contact email.",
        ));
    }
    let results = match quota_replies::respond_to_open_quota_tickets(
        &quota_replies::SystemAzRunner,
        &email,
        dry_run,
        false,
    ) {
        Ok(results) => results,
        Err(err) => {
            if let Some(cmd) = support_permission_error(&err) {
                return Err(cmd);
            }
            return Err(CmdError::click(err.to_string()));
        }
    };
    if results.is_empty() {
        println!("(no Open Azure quota tickets requiring reply)");
        return Ok(());
    }
    println!("{:<46} {:<22} {:<3} ACTION", "TICKET", "REGION", "OK");
    println!("{}", "-".repeat(92));
    let mut ok_count = 0;
    for r in &results {
        let ok = r.get("ok").and_then(Value::as_bool).unwrap_or(false);
        if ok {
            ok_count += 1;
        }
        let name = r.get("name").and_then(Value::as_str).unwrap_or("?");
        let region = r.get("region").and_then(Value::as_str).unwrap_or("-");
        let region = if region.is_empty() { "-" } else { region };
        let action = r.get("action").and_then(Value::as_str).unwrap_or("?");
        let detail = match r.get("error").and_then(Value::as_str) {
            Some(error) if !error.is_empty() => format!(" — {error}"),
            _ => String::new(),
        };
        println!(
            "{:<46} {:<22} {:<3} {action}{detail}",
            take(name, 44),
            take(region, 20),
            if ok { "Y" } else { "N" },
        );
    }
    println!("\n{ok_count}/{} tickets processed", results.len());
    Ok(())
}

/// Python `quota_azure_escalate`: post the credit-funded-subscription
/// escalation on billing-decline tickets.
async fn azure_escalate(dry_run: bool, email_arg: &str) -> Result<(), CmdError> {
    let email = contact_email(email_arg);
    if email.is_empty() {
        return Err(CmdError::click(
            "--email is required (or set WC_QUOTA_CONTACT_EMAIL); the \
             escalation message signs off with the customer contact email.",
        ));
    }
    let results = match quota_replies::respond_to_open_quota_tickets(
        &quota_replies::SystemAzRunner,
        &email,
        dry_run,
        true,
    ) {
        Ok(results) => results,
        Err(err) => {
            if let Some(cmd) = support_permission_error(&err) {
                return Err(cmd);
            }
            return Err(CmdError::click(err.to_string()));
        }
    };
    // Filter to rows that represent an escalation outcome only. Dry-run
    // rows carry a `would` field that says "escalated" vs "replied" —
    // the standard reply path is what azure-replies handles, so this
    // CLI surfaces only the billing-decline → escalation rows.
    let relevant: Vec<&Value> = results
        .iter()
        .filter(|r| {
            let action = r.get("action").and_then(Value::as_str).unwrap_or("");
            action == "escalated"
                || action == "error"
                || (action == "dry_run"
                    && r.get("would").and_then(Value::as_str) == Some("escalated"))
        })
        .collect();
    if relevant.is_empty() {
        println!("(no billing-decline tickets to escalate)");
        return Ok(());
    }
    println!("{:<46} {:<22} {:<3} ACTION", "TICKET", "REGION", "OK");
    println!("{}", "-".repeat(92));
    let mut ok_count = 0;
    for r in &relevant {
        let ok = r.get("ok").and_then(Value::as_bool).unwrap_or(false);
        if ok {
            ok_count += 1;
        }
        let name = r.get("name").and_then(Value::as_str).unwrap_or("?");
        let region = r.get("region").and_then(Value::as_str).unwrap_or("-");
        let region = if region.is_empty() { "-" } else { region };
        let mut action = r
            .get("action")
            .and_then(Value::as_str)
            .unwrap_or("?")
            .to_string();
        if action == "dry_run" {
            action = "dry_run → would escalate".to_string();
        }
        let detail = match r.get("error").and_then(Value::as_str) {
            Some(error) if !error.is_empty() => format!(" — {error}"),
            _ => String::new(),
        };
        println!(
            "{:<46} {:<22} {:<3} {action}{detail}",
            take(name, 44),
            take(region, 20),
            if ok { "Y" } else { "N" },
        );
    }
    println!(
        "\n{ok_count}/{} billing-decline tickets escalated",
        relevant.len()
    );
    Ok(())
}
