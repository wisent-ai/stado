//! The billing section: what each provider says has already been spent, and
//! how fast the credits are going.
//!
//! Both providers get an explicit unavailable line rather than silence,
//! because a missing credit balance is itself the operator's news.

use serde_json::Value;

use super::amounts::{dollars, figure, Figure};

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
    let credit = |value: Option<&Value>| figure(value).credited();
    if gcp.get("status").and_then(Value::as_str) == Some("ok") {
        // A figure the report does not carry is said as not reported, never
        // as a zero bill or a month nobody read.
        match gcp
            .get("monthly")
            .and_then(Value::as_array)
            .map(|rows| rows.last())
        {
            Some(Some(month)) => println!(
                "  GCP {}: gross={} credits={} net={}",
                match month.get("month").and_then(Value::as_str) {
                    Some(month) => month,
                    None => "(month not reported)",
                },
                dollars(&figure(month.get("gross"))),
                dollars(&credit(month.get("credits"))),
                dollars(&figure(month.get("net"))),
            ),
            Some(None) => println!("  GCP monthly: no month reported yet"),
            None => println!("  GCP monthly: not reported"),
        }
        println!(
            "  GCP credit burn (7d avg): {}/day",
            dollars(&credit(gcp.get("avg_daily_credit_applied_7d")))
        );
        println!(
            "  GCP promotion credits applied: {}",
            promotion_applied(gcp)
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
        // A figure Azure's answer does not carry is said as not reported,
        // never as a word or a currency nobody reported.
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
        let said = |key: &str| match azure.get(key).and_then(Value::as_str) {
            Some(text) => text.to_string(),
            None => format!("({key} not reported)"),
        };
        println!("  Azure credits: {} — {}", said("status"), said("detail"));
    }
}

/// The promotion credits GCP applied, summed over every promotion credit it
/// lists; a credit list it does not carry, or a promotion whose cumulative
/// is absent or unreadable, is said as such instead of summed as zero.
fn promotion_applied(gcp: &Value) -> String {
    let Some(credits) = gcp.get("credits").and_then(Value::as_array) else {
        return dollars(&Figure::Absent);
    };
    let figures: Vec<Figure> = credits
        .iter()
        .filter(|entry| entry.get("type").and_then(Value::as_str) == Some("PROMOTION"))
        .map(|entry| figure(entry.get("cumulative")).credited())
        .collect();
    let amounts: Vec<f64> = figures
        .iter()
        .filter_map(|figure| match figure {
            Figure::Amount(amount) => Some(*amount),
            Figure::Absent | Figure::Unreadable(_) => None,
        })
        .collect();
    match figures
        .iter()
        .find(|figure| !matches!(figure, Figure::Amount(_)))
    {
        Some(unread) => format!("a promotion credit's cumulative is {}", dollars(unread)),
        None => dollars(&Figure::Amount(amounts.into_iter().sum())),
    }
}
