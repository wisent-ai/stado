//! The Azure half of `billing show`: the credit balance, the grant window
//! it is drawn from, the spending-limit warning that says charges will
//! continue once it is gone, and whether the provisioning credit gate admits
//! a VM create or start on that balance.

use serde_json::Value;

use crate::cli::reporting::field;

pub(super) fn print_azure(section: &Value) {
    if section.get("status").and_then(Value::as_str) != Some("ok") {
        println!(
            "Azure: {} — {}",
            field(&section["status"]),
            field(&section["detail"])
        );
        print_gate(section);
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
    print_gate(section);
}

/// The provisioning credit gate the collector decided on this section; a
/// document written before the gate existed carries none, which is said.
fn print_gate(section: &Value) {
    let gate = &section["provision_gate"];
    match gate.get("admits").and_then(Value::as_bool) {
        Some(true) => println!(
            "Azure provisioning gate: the balance is above the floor {} {} (the operator's ruling {}: \"{}\"); {}",
            field(&gate["floor"]["amount"]),
            field(&gate["floor"]["currency"]),
            field(&gate["floor"]["ruling"]),
            field(&gate["floor"]["said"]),
            field(&gate["perStart"])
        ),
        Some(false) => println!("Azure provisioning gate: {}", field(&gate["refusal"])),
        None => println!(
            "Azure provisioning gate: this billing document carries no gate decision; `stado billing refresh` writes one"
        ),
    }
}
