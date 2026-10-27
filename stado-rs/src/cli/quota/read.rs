//! The READ side of `stado quota`: `show` prints live cloud quota minus
//! reservation minus running per provider, and `catalog` prints the full
//! GPU catalog each provider's quota adapter reports. Neither writes
//! anything; both accept the group-level or subcommand-level `--json`.

use serde_json::Value;

use super::common::{echo_json, parse_providers, quota_adapter};
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
        .map_err(CmdError::from)?;
    if as_json {
        echo_json(&serde_json::to_value(&summary)?);
        return Ok(());
    }
    let header = format!(
        "{:<10} {:<22} {:>6} {:>9} {:>5} {:>6}",
        "PROVIDER", "ACCEL", "TOTAL", "RESERVED", "USED", "AVAIL"
    );
    println!("{header}");
    let rule = "-".repeat(header.chars().count());
    println!("{rule}");
    // Per accelerator across providers: the total and the available count,
    // summed only from rows a provider gave.
    let mut grand: std::collections::BTreeMap<String, (i64, u64)> = Default::default();
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
            let (total, available) = grand.entry(accel.clone()).or_default();
            *total += row.total;
            *available += row.available;
        }
    }
    // The grand total says something only when a second provider answered
    // beside the first.
    let mut providers = summary.keys();
    let several = providers.next().is_some() && providers.next().is_some();
    if several && !grand.is_empty() {
        println!("{rule}");
        for (accel, (total, avail)) in &grand {
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
        .map_err(CmdError::from)?;
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
                    match row.get("error").and_then(Value::as_str) {
                        Some(error) => println!("  ERROR: {error}"),
                        None => println!("  ERROR: the provider row failed without an error text"),
                    }
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
            // A row without its quota id or region sorts ahead of those with
            // one, rather than under an empty name.
            sorted.sort_by(|left, right| {
                (left["quota_id"].as_str(), left["region"].as_str())
                    .cmp(&(right["quota_id"].as_str(), right["region"].as_str()))
            });
            for row in sorted {
                let quota_id = match row.get("quota_id").and_then(Value::as_str) {
                    Some(quota_id) if !quota_id.is_empty() => quota_id,
                    _ => crate::cli::reporting::NOT_REPORTED,
                };
                // A quota without a family or region dimension says so, and a
                // limit GCP did not give reads as not reported; a dash read
                // as a value of its own.
                let family = match row.get("gpu_family").and_then(Value::as_str) {
                    Some(family) if !family.is_empty() => family,
                    _ => "(no gpu family)",
                };
                let region = match row.get("region").and_then(Value::as_str) {
                    Some(region) if !region.is_empty() => region,
                    _ => "(no region)",
                };
                let limit = match row.get("limit") {
                    Some(Value::Number(n)) => n.to_string(),
                    _ => crate::cli::reporting::NOT_REPORTED.to_string(),
                };
                println!(
                    "  {:<52} {:<20} {:<16} {:>6}",
                    quota_id, family, region, limit
                );
            }
        } else if quota_adapter(provider) == Some(crate::capabilities::QuotaAdapter::Azure) {
            let mut seen_fam: std::collections::BTreeMap<
                String,
                std::collections::BTreeSet<String>,
            > = Default::default();
            for row in rows {
                // A row without its family or location is grouped and counted
                // under that statement, not under an empty name.
                let family = match row.get("family").and_then(Value::as_str) {
                    Some(family) => family,
                    None => "(family not reported)",
                };
                let location = match row.get("location").and_then(Value::as_str) {
                    Some(location) => location,
                    None => "(location not reported)",
                };
                seen_fam
                    .entry(family.to_string())
                    .or_default()
                    .insert(location.to_string());
            }
            println!("  {:<36} LOCATIONS", "FAMILY");
            for (family, locations) in &seen_fam {
                let locs: Vec<&str> = locations.iter().map(String::as_str).collect();
                println!("  {:<36} {} ({})", family, locs.len(), locs.join(", "));
            }
        }
    }
    Ok(())
}
