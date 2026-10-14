//! The billing section: what each provider says has already been spent, and
//! how fast the credits are going.
//!
//! Both providers get an explicit unavailable line rather than silence,
//! because a missing credit balance is itself the operator's news.

use serde_json::Value;

use super::amounts::{amount, dollars};

pub(super) fn print_billing(document: &Value) {
    println!("billing:");
    let billing = &document["billing"];
    println!(
        "  reported: {}",
        match billing.get("reported_at").and_then(Value::as_str) {
            Some(at) => at,
            None => "not reported",
        }
    );
    let gcp = &billing[crate::capabilities::ProviderId::Gcp.as_str()];
    let credit = |value: Option<&Value>| amount(value).map(|credit| -credit);
    if gcp.get("status").and_then(Value::as_str) == Some("ok") {
        // A figure the report does not carry is said as not reported; 0.00
        // and the word "current" stood in for a bill and a month nobody read.
        if let Some(month) = gcp
            .get("monthly")
            .and_then(Value::as_array)
            .and_then(|rows| rows.last())
        {
            println!(
                "  GCP {}: gross={} credits={} net={}",
                match month.get("month").and_then(Value::as_str) {
                    Some(month) => month,
                    None => "(month not reported)",
                },
                dollars(amount(month.get("gross"))),
                dollars(credit(month.get("credits"))),
                dollars(amount(month.get("net"))),
            );
        }
        println!(
            "  GCP credit burn (7d avg): {}/day",
            dollars(credit(gcp.get("avg_daily_credit_applied_7d")))
        );
        let promotion_used = gcp
            .get("credits")
            .and_then(Value::as_array)
            .and_then(|credits| {
                credits
                    .iter()
                    .filter(|entry| entry.get("type").and_then(Value::as_str) == Some("PROMOTION"))
                    .map(|entry| credit(entry.get("cumulative")))
                    .sum::<Option<f64>>()
            });
        println!(
            "  GCP promotion credits applied: {}",
            dollars(promotion_used)
        );
        println!("  GCP promotion remaining: unavailable (grant ceiling is not exposed by GCP)");
    } else {
        match gcp.get("detail").and_then(Value::as_str) {
            Some(detail) => println!("  GCP: {detail}"),
            None => println!("  GCP: unavailable, and the report gives no reason: {gcp}"),
        }
    }
    let azure = &billing[crate::capabilities::ProviderId::Azure.as_str()];
    if azure.get("status").and_then(Value::as_str) == Some("ok") {
        // A figure Azure's answer does not carry is said as not reported; the
        // word "unknown" and a currency of USD were both stand-ins nobody
        // reported.
        let figure = |key: &str| match azure.get(key) {
            Some(Value::String(text)) => text.clone(),
            Some(value) if !value.is_null() => value.to_string(),
            _ => "not reported".to_string(),
        };
        println!(
            "  Azure credits: current={} estimated={} {}",
            figure("available_balance"),
            figure("estimated_balance"),
            match azure.get("currency").and_then(Value::as_str) {
                Some(currency) => currency.to_string(),
                None => "(currency not reported)".to_string(),
            },
        );
        println!(
            "  Azure grant: amount={} used={}, valid {} — {}",
            figure("grant_amount"),
            figure("credit_used"),
            figure("grant_start_date"),
            figure("grant_end_date"),
        );
        println!(
            "  Azure pending eligible charges={} expired={}",
            figure("pending_eligible_charges"),
            figure("expired_credit"),
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
