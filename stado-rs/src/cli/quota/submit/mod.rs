//! The WRITE side of `stado quota`: what asks a provider for more
//! capacity. [`increase`] submits the quota-increase requests themselves
//! (`request`, `request-all`); the Azure support-ticket flows that carry
//! a submitted request the rest of the way — `replies --provider azure`
//! answering the tickets awaiting customer info, `escalate --provider azure`
//! posting the credit-funded-subscription escalation on billing declines — stay here.
//! An `az` failure is reported with az's own stderr; it is not sorted by its
//! words.

// ---- write-side subcommands ----

mod increase;

pub(super) use increase::{request, request_all};

use serde_json::Value;

use super::common::{contact_email, take};
use crate::cli::CmdError;
use crate::scheduler::dispatch::quota_replies;

/// Python `quota_azure_replies`: respond to Open Azure quota tickets
/// awaiting customer info.
pub(super) async fn azure_replies(dry_run: bool, email_arg: &str) -> Result<(), CmdError> {
    let email = contact_email(email_arg);
    if email.is_empty() {
        return Err(CmdError::refused(
            "--email is required (or set WC_QUOTA_CONTACT_EMAIL); the \
             reply body signs off with the customer contact email.",
        ));
    }
    let results = match quota_replies::respond_to_open_quota_tickets(
        &quota_replies::SystemAzRunner,
        &email,
        dry_run,
        false,
    ) {
        Ok(results) => results,
        Err(err) => return Err(CmdError::click(err.to_string())),
    };
    if results.is_empty() {
        println!("(no Open Azure quota tickets requiring reply)");
        return Ok(());
    }
    println!("{:<46} {:<22} {:<3} ACTION", "TICKET", "REGION", "OK");
    println!("{}", "-".repeat(92));
    let mut ok_count = 0;
    for r in &results {
        let ok = r.get("ok").and_then(Value::as_bool).unwrap_or(false);
        if ok {
            ok_count += 1;
        }
        let name = r.get("name").and_then(Value::as_str).unwrap_or("?");
        let region = r.get("region").and_then(Value::as_str).unwrap_or("-");
        let region = if region.is_empty() { "-" } else { region };
        let action = r.get("action").and_then(Value::as_str).unwrap_or("?");
        let detail = match r.get("error").and_then(Value::as_str) {
            Some(error) if !error.is_empty() => format!(" — {error}"),
            _ => String::new(),
        };
        println!(
            "{:<46} {:<22} {:<3} {action}{detail}",
            take(name, 44),
            take(region, 20),
            if ok { "Y" } else { "N" },
        );
    }
    println!("\n{ok_count}/{} tickets processed", results.len());
    Ok(())
}

/// Python `quota_azure_escalate`: post the credit-funded-subscription
/// escalation on billing-decline tickets.
pub(super) async fn azure_escalate(dry_run: bool, email_arg: &str) -> Result<(), CmdError> {
    let email = contact_email(email_arg);
    if email.is_empty() {
        return Err(CmdError::refused(
            "--email is required (or set WC_QUOTA_CONTACT_EMAIL); the \
             escalation message signs off with the customer contact email.",
        ));
    }
    let results = match quota_replies::respond_to_open_quota_tickets(
        &quota_replies::SystemAzRunner,
        &email,
        dry_run,
        true,
    ) {
        Ok(results) => results,
        Err(err) => return Err(CmdError::click(err.to_string())),
    };
    // Filter to rows that represent an escalation outcome only. Dry-run
    // rows carry a `would` field that says "escalated" vs "replied" —
    // the standard reply path is what `quota replies` handles, so this
    // CLI surfaces only the billing-decline → escalation rows.
    let relevant: Vec<&Value> = results
        .iter()
        .filter(|r| {
            let action = r.get("action").and_then(Value::as_str).unwrap_or("");
            action == "escalated"
                || action == "error"
                || (action == "dry_run"
                    && r.get("would").and_then(Value::as_str) == Some("escalated"))
        })
        .collect();
    if relevant.is_empty() {
        println!("(no billing-decline tickets to escalate)");
        return Ok(());
    }
    println!("{:<46} {:<22} {:<3} ACTION", "TICKET", "REGION", "OK");
    println!("{}", "-".repeat(92));
    let mut ok_count = 0;
    for r in &relevant {
        let ok = r.get("ok").and_then(Value::as_bool).unwrap_or(false);
        if ok {
            ok_count += 1;
        }
        let name = r.get("name").and_then(Value::as_str).unwrap_or("?");
        let region = r.get("region").and_then(Value::as_str).unwrap_or("-");
        let region = if region.is_empty() { "-" } else { region };
        let mut action = r
            .get("action")
            .and_then(Value::as_str)
            .unwrap_or("?")
            .to_string();
        if action == "dry_run" {
            action = "dry_run → would escalate".to_string();
        }
        let detail = match r.get("error").and_then(Value::as_str) {
            Some(error) if !error.is_empty() => format!(" — {error}"),
            _ => String::new(),
        };
        println!(
            "{:<46} {:<22} {:<3} {action}{detail}",
            take(name, 44),
            take(region, 20),
            if ok { "Y" } else { "N" },
        );
    }
    println!(
        "\n{ok_count}/{} billing-decline tickets escalated",
        relevant.len()
    );
    Ok(())
}
