//! The READ side of `stado quota`: `show` prints live cloud quota minus
//! reservation minus running per provider, and `catalog` prints the full
//! GPU catalog each provider's quota adapter reports. Neither writes
//! anything; both accept the group-level or subcommand-level `--json`.

use serde_json::Value;

use super::common::{echo_json, parse_providers, quota_adapter, take};
use crate::cli::CmdError;
use crate::queue::JobStorage;
use crate::scheduler::dispatch::quota_skus;
use crate::scheduler::quota;

/// Python `quota_show`: table of live cloud quota minus reservation minus
/// running per provider (or --json).
pub(super) async fn show(as_json: bool) -> Result<(), CmdError> {
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
pub(super) async fn catalog(providers_arg: &str, as_json: bool) -> Result<(), CmdError> {
    let providers = parse_providers(providers_arg)?;
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
        if quota_adapter(provider) == Some(crate::capabilities::QuotaAdapter::Gcp) {
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
        } else if quota_adapter(provider) == Some(crate::capabilities::QuotaAdapter::Azure) {
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
