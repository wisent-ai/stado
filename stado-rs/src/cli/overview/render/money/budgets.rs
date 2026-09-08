//! The budgets section: the policy limits beside the current burn, and the
//! GCP budgets the billing account actually carries.
//!
//! A detached GCP billing account prints nothing here rather than an error:
//! it is a configuration choice, not a failure.

use serde_json::Value;

use super::amounts::{money, number};

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
        println!(
            "  burn: {} USD/h now, {} USD projected at month end, budget exceeded={}",
            number(policy.get("current_hourly_usd")),
            number(policy.get("end_of_month_usd")),
            policy
                .get("budget_exceeded")
                .and_then(Value::as_bool)
                .map_or("unknown".to_string(), |value| value.to_string()),
        );
        if let Some(days) = policy.get("credit_runway_days").and_then(Value::as_f64) {
            println!("  credit runway: {days:.1} days");
        }
    } else {
        println!(
            "  policy: {}",
            policy
                .get("detail")
                .and_then(Value::as_str)
                .unwrap_or("unknown error")
        );
    }
    let gcp = &document["budgets"]["gcp"];
    match gcp.get("status").and_then(Value::as_str) {
        Some("ok") => {
            let rows = gcp
                .get("budgets")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
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
        _ => println!(
            "  GCP: unavailable: {}",
            gcp.get("detail")
                .and_then(Value::as_str)
                .unwrap_or("unknown error")
        ),
    }
}
