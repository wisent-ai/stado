//! The budgets section: the policy limits beside the current burn, and the
//! GCP budgets the billing account actually carries.
//!
//! A detached GCP billing account prints nothing here rather than an error:
//! it is a configuration choice, not a failure.

use serde_json::Value;

use super::amounts::{dollars, figure, money, Figure};

pub(super) fn print_budgets(document: &Value) {
    println!("budgets:");
    let policy = &document["budgets"]["policy"];
    if policy.get("status").and_then(Value::as_str) == Some("ok") {
        // An absent or null limit is one the policy does not set; one it sets
        // in no numeric shape is said as unreadable, not read as zero.
        let limit = |key: &str| match figure(policy.get(key)) {
            Figure::Absent => "not set".to_string(),
            Figure::Amount(value) => format!("USD {value:.2}"),
            unread => dollars(&unread),
        };
        println!(
            "  policy: hourly {} | daily {} | monthly {}",
            limit("hourly_usd"),
            limit("daily_usd"),
            limit("monthly_usd"),
        );
        let usd = |key: &str| match figure(policy.get(key)) {
            Figure::Amount(value) => format!("{value} USD"),
            unread => dollars(&unread),
        };
        println!(
            "  burn: {}/h now, {} projected at month end, budget exceeded={}",
            usd("current_hourly_usd"),
            usd("end_of_month_usd"),
            crate::cli::reporting::field(&policy["budget_exceeded"]),
        );
        match figure(policy.get("credit_runway_days")) {
            Figure::Absent => {}
            Figure::Amount(days) => println!("  credit runway: {days:.1} days"),
            Figure::Unreadable(days) => println!("  credit runway: unreadable ({days})"),
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
                println!(
                    "  GCP {}: {} ({})",
                    name(budget),
                    amount(budget),
                    period(budget)
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

/// A budget's display name, else its resource name: the API sets the
/// resource name on every budget and the display name only when one was
/// given.
fn name(budget: &Value) -> String {
    match (
        budget.get("displayName").and_then(Value::as_str),
        budget.get("name").and_then(Value::as_str),
    ) {
        (Some(display), _) => display.to_string(),
        (None, Some(resource)) => resource.to_string(),
        (None, None) => format!("(a budget naming itself nothing: {budget})"),
    }
}

/// A budget's amount: a specified `Money`, or the last period's spend, the
/// two kinds the Budget API carries.
fn amount(budget: &Value) -> String {
    match (
        budget.pointer("/amount/specifiedAmount"),
        budget.pointer("/amount/lastPeriodAmount"),
    ) {
        (Some(specified), _) => money(specified),
        (None, Some(_)) => "the last period's spend".to_string(),
        (None, None) => "amount not reported".to_string(),
    }
}

/// The period a budget tracks: a calendar period, or a custom period from
/// its start date, as the Budget API's filter carries one or the other.
fn period(budget: &Value) -> String {
    match (
        budget
            .pointer("/budgetFilter/calendarPeriod")
            .and_then(Value::as_str),
        budget.pointer("/budgetFilter/customPeriod"),
    ) {
        (Some(calendar), _) => calendar.to_string(),
        (None, Some(custom)) => format!("custom period {custom}"),
        (None, None) => "period not reported".to_string(),
    }
}
