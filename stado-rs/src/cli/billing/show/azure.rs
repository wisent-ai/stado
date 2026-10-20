//! The Azure half of `billing show`: the credit balance, the grant window
//! it is drawn from, and the spending-limit warning that says charges will
//! continue once it is gone.

use serde_json::Value;

use crate::cli::reporting::field;

pub(super) fn print_azure(section: &Value) {
    if section.get("status").and_then(Value::as_str) != Some("ok") {
        println!(
            "Azure: {} — {}",
            field(&section["status"]),
            field(&section["detail"])
        );
        return;
    }
    println!(
        "Azure credits: current={} estimated={} {}",
        field(&section["available_balance"]),
        field(&section["estimated_balance"]),
        field(&section["currency"])
    );
    println!(
        "Azure grant: amount={} used={}, valid {} — {}",
        field(&section["grant_amount"]),
        field(&section["credit_used"]),
        field(&section["grant_start_date"]),
        field(&section["grant_end_date"]),
    );
    println!(
        "Azure pending eligible charges={} expired={}",
        field(&section["pending_eligible_charges"]),
        field(&section["expired_credit"]),
    );
    if section.get("overage_risk").and_then(Value::as_bool) == Some(true) {
        println!("Azure warning: spending limit is off; charges may continue after credits are exhausted");
    }
}
