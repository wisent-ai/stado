//! The budgets section: the policy limits beside the current burn, and the
//! GCP budgets the billing account actually carries.
//!
//! A detached GCP billing account prints nothing here rather than an error:
//! it is a configuration choice, not a failure.

use serde_json::Value;

use super::amounts::{amount, money};

pub(super) fn print_budgets(document: &Value) {
    println!("budgets:");
    let policy = &document["budgets"]["policy"];
    if policy.get("status").and_then(Value::as_str) == Some("ok") {
        let limit = |key: &str| match policy.get(key) {
            Some(Value::Number(value)) => format!("USD {:.2}", value.as_f64().unwrap_or_default()),
            _ => "not set".to_string(),
        };
        println!(
            "  policy: hourly {} | daily {} | monthly {}",
            limit("hourly_usd"),
            limit("daily_usd"),
            limit("monthly_usd"),
        );
        let usd = |key: &str| match amount(policy.get(key)) {
            Some(value) => format!("{value} USD"),
            None => "not reported".to_string(),
        };
        println!(
            "  burn: {}/h now, {} projected at month end, budget exceeded={}",
            usd("current_hourly_usd"),
            usd("end_of_month_usd"),
            match policy.get("budget_exceeded").and_then(Value::as_bool) {
                Some(exceeded) => exceeded.to_string(),
                None => "not reported".to_string(),
            },
        );
        if let Some(days) = policy.get("credit_runway_days").and_then(Value::as_f64) {
            println!("  credit runway: {days:.1} days");
        }
    } else {
        match policy.get("detail").and_then(Value::as_str) {
            Some(detail) => println!("  policy: {detail}"),
            None => println!("  policy: unavailable, and the report gives no reason: {policy}"),
        }
    }
    let gcp = &document["budgets"]["gcp"];
    match gcp.get("status").and_then(Value::as_str) {
        Some("ok") => {
            // `ok` with no budgets list is a report this reader cannot read,
            // not a billing account with no budgets on it.
            let Some(rows) = gcp.get("budgets").and_then(Value::as_array) else {
                println!("  GCP: the report says ok but carries no budgets list: {gcp}");
                return;
            };
            if rows.is_empty() {
                println!("  GCP: no budgets configured");
            }
            for budget in rows {
                let name = budget
                    .get("displayName")
                    .and_then(Value::as_str)
                    .unwrap_or("unnamed");
                let period = budget
                    .pointer("/budgetFilter/calendarPeriod")
                    .and_then(Value::as_str)
                    .unwrap_or("CUSTOM");
                let amount = budget
                    .pointer("/amount/specifiedAmount")
                    .or_else(|| budget.pointer("/amount/lastPeriodAmount"));
                println!(
                    "  GCP {name}: {} ({period})",
                    amount.map_or_else(|| "dynamic amount".to_string(), money)
                );
            }
        }
        Some("detached") => {}
        _ => match gcp.get("detail").and_then(Value::as_str) {
            Some(detail) => println!("  GCP: unavailable: {detail}"),
            None => println!("  GCP: unavailable, and the report gives no reason: {gcp}"),
        },
    }
}
