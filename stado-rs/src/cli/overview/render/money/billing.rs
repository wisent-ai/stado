//! The billing section: what each provider says has already been spent, and
//! how fast the credits are going.
//!
//! Both providers get an explicit unavailable line rather than silence,
//! because a missing credit balance is itself the operator's news.

use serde_json::Value;

use super::amounts::number;

pub(super) fn print_billing(document: &Value) {
    println!("billing:");
    let billing = &document["billing"];
    println!(
        "  reported: {}",
        billing
            .get("reported_at")
            .and_then(Value::as_str)
            .unwrap_or("unavailable")
    );
    let gcp = &billing[crate::capabilities::ProviderId::Gcp.as_str()];
    if gcp.get("status").and_then(Value::as_str) == Some("ok") {
        if let Some(month) = gcp
            .get("monthly")
            .and_then(Value::as_array)
            .and_then(|rows| rows.last())
        {
            println!(
                "  GCP {}: gross=${:.2} credits=${:.2} net=${:.2}",
                month
                    .get("month")
                    .and_then(Value::as_str)
                    .unwrap_or("current"),
                number(month.get("gross")),
                -number(month.get("credits")),
                number(month.get("net")),
            );
        }
        println!(
            "  GCP credit burn (7d avg): ${:.2}/day",
            -number(gcp.get("avg_daily_credit_applied_7d"))
        );
        let promotion_used: f64 = gcp
            .get("credits")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter(|credit| credit.get("type").and_then(Value::as_str) == Some("PROMOTION"))
            .map(|credit| -number(credit.get("cumulative")))
            .sum();
        println!("  GCP promotion credits applied: ${promotion_used:.2}");
        println!("  GCP promotion remaining: unavailable (grant ceiling is not exposed by GCP)");
    } else {
        println!(
            "  GCP: {}",
            gcp.get("detail")
                .and_then(Value::as_str)
                .unwrap_or("unavailable")
        );
    }
    let azure = &billing[crate::capabilities::ProviderId::Azure.as_str()];
    if azure.get("status").and_then(Value::as_str) == Some("ok") {
        println!(
            "  Azure credits: current={} estimated={} {}",
            azure
                .get("available_balance")
                .map_or_else(|| "unknown".to_string(), Value::to_string),
            azure
                .get("estimated_balance")
                .map_or_else(|| "unknown".to_string(), Value::to_string),
            azure
                .get("currency")
                .and_then(Value::as_str)
                .unwrap_or("USD"),
        );
        println!(
            "  Azure grant: amount={} used={}, valid {} — {}",
            azure
                .get("grant_amount")
                .map_or_else(|| "unknown".to_string(), Value::to_string),
            azure
                .get("credit_used")
                .map_or_else(|| "unknown".to_string(), Value::to_string),
            azure
                .get("grant_start_date")
                .and_then(Value::as_str)
                .unwrap_or("unknown"),
            azure
                .get("grant_end_date")
                .and_then(Value::as_str)
                .unwrap_or("unknown"),
        );
        println!(
            "  Azure pending eligible charges={} expired={}",
            azure
                .get("pending_eligible_charges")
                .map_or_else(|| "unknown".to_string(), Value::to_string),
            azure
                .get("expired_credit")
                .map_or_else(|| "unknown".to_string(), Value::to_string),
        );
        if azure.get("overage_risk").and_then(Value::as_bool) == Some(true) {
            println!(
                "  Azure warning: spending limit is off; paid overage can continue after credits"
            );
        }
    } else {
        println!(
            "  Azure credits: {} — {}",
            azure
                .get("status")
                .and_then(Value::as_str)
                .unwrap_or("unavailable"),
            azure
                .get("detail")
                .and_then(Value::as_str)
                .unwrap_or("no detail"),
        );
    }
}
