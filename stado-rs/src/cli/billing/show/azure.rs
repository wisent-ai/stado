//! The Azure half of `billing show`: the credit balance, the grant window
//! it is drawn from, and the spending-limit warning that says charges will
//! continue once it is gone.

use serde_json::Value;

use crate::cli::billing::format::text;

pub(super) fn print_azure(section: &Value) {
    if section.get("status").and_then(Value::as_str) != Some("ok") {
        println!(
            "Azure: {} — {}",
            section
                .get("status")
                .and_then(Value::as_str)
                .unwrap_or("unavailable"),
            section
                .get("detail")
                .and_then(Value::as_str)
                .unwrap_or("no detail")
        );
        return;
    }
    println!(
        "Azure credits: current={} estimated={} {}",
        text(section.get("available_balance")),
        text(section.get("estimated_balance")),
        section
            .get("currency")
            .and_then(Value::as_str)
            .unwrap_or("USD")
    );
    println!(
        "Azure grant: amount={} used={}, valid {} — {}",
        text(section.get("grant_amount")),
        text(section.get("credit_used")),
        text(section.get("grant_start_date")),
        text(section.get("grant_end_date")),
    );
    println!(
        "Azure pending eligible charges={} expired={}",
        text(section.get("pending_eligible_charges")),
        text(section.get("expired_credit")),
    );
    if section.get("overage_risk").and_then(Value::as_bool) == Some(true) {
        println!("Azure warning: spending limit is off; charges may continue after credits are exhausted");
    }
}
