//! The WRITE side of `stado quota`: what asks a provider for more
//! capacity. [`increase`] submits the quota-increase requests themselves
//! (`request create`); the Azure support-ticket flows that carry
//! a submitted request the rest of the way — `ticket reply --provider azure`
//! answering the tickets awaiting customer info, `ticket escalate --provider azure`
//! posting the credit-funded-subscription escalation on billing declines — stay here.
//! An `az` failure is reported with az's own stderr; it is not sorted by its
//! words.

// ---- write-side subcommands ----

mod increase;

pub(super) use increase::{request, request_all};

use serde_json::Value;

use super::common::contact_email;
use crate::cli::reporting::{field, NOT_REPORTED};
use crate::cli::CmdError;
use crate::scheduler::dispatch::quota_replies;

/// The table both ticket flows print, one row per provider result, and how
/// many of them the provider said succeeded. A field the result does not
/// carry reads `not reported`; a result that does not say whether it
/// succeeded is neither counted nor shown as a failure.
fn print_results(rows: &[&Value]) -> usize {
    let header = format!("{:<46} {:<22} {:<3} ACTION", "TICKET", "REGION", "OK");
    println!("{header}");
    println!("{}", "-".repeat(header.chars().count()));
    for row in rows {
        let region = match row["region"].as_str() {
            Some(region) if !region.is_empty() => region,
            _ => NOT_REPORTED,
        };
        let ok = match row["ok"].as_bool() {
            Some(true) => "Y",
            Some(false) => "N",
            None => NOT_REPORTED,
        };
        let action = match (row["action"].as_str(), row["would"].as_str()) {
            (Some("dry_run"), Some(would)) => format!("dry_run → would be {would}"),
            _ => field(&row["action"]),
        };
        let detail = match row["error"].as_str() {
            Some(error) if !error.is_empty() => format!(" — {error}"),
            _ => String::new(),
        };
        println!(
            "{:<46} {:<22} {:<3} {action}{detail}",
            field(&row["name"]),
            region,
            ok
        );
    }
    rows.iter()
        .filter(|row| row["ok"].as_bool() == Some(true))
        .count()
}

/// Python `quota_azure_replies`: respond to Open Azure quota tickets
/// awaiting customer info.
pub(super) async fn azure_replies(dry_run: bool, email_arg: &str) -> Result<(), CmdError> {
    let email = contact_email(email_arg)?;
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
        Err(err) => return Err(CmdError::from(err)),
    };
    if results.is_empty() {
        println!("(no Open Azure quota tickets requiring reply)");
        return Ok(());
    }
    let rows = results.iter().collect::<Vec<_>>();
    let succeeded = print_results(&rows);
    println!("\n{succeeded}/{} tickets processed", results.len());
    Ok(())
}

/// Python `quota_azure_escalate`: post the credit-funded-subscription
/// escalation on billing-decline tickets.
pub(super) async fn azure_escalate(dry_run: bool, email_arg: &str) -> Result<(), CmdError> {
    let email = contact_email(email_arg)?;
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
        Err(err) => return Err(CmdError::from(err)),
    };
    // Filter to rows that represent an escalation outcome only. Dry-run
    // rows carry a `would` field that says "escalated" vs "replied" —
    // the standard reply path is what `quota ticket reply` handles, so this
    // CLI surfaces only the billing-decline → escalation rows.
    let relevant: Vec<&Value> = results
        .iter()
        .filter(|r| match r.get("action").and_then(Value::as_str) {
            // A row without its action cannot be placed; read as "", it was
            // dropped and could leave "(no billing-decline tickets)" printed
            // over a ticket nobody saw. It is shown with its action unreported.
            None => true,
            Some(action) => {
                action == "escalated"
                    || action == "error"
                    || (action == "dry_run"
                        && r.get("would").and_then(Value::as_str) == Some("escalated"))
            }
        })
        .collect();
    if relevant.is_empty() {
        println!("(no billing-decline tickets to escalate)");
        return Ok(());
    }
    let succeeded = print_results(&relevant);
    println!(
        "\n{succeeded}/{} billing-decline tickets escalated",
        relevant.len()
    );
    Ok(())
}
