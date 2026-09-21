//! What `pending`, `status` and `failures` print.
//!
//! Three questions, three answers: what has been written and not proven, how
//! the passes went, and which failures are still waiting to be handed back to
//! the task that caused them.

use serde_json::json;

use crate::cli::delivery::{print_json, read_registry};
use crate::cli::CmdError;
use crate::targets::{read_deliveries, read_passes, Delivery, DeliveryState, QualificationPass};

/// Deliveries nobody has proven yet, oldest first — the queue a pass drains.
pub(super) async fn pending(product: Option<String>, json_output: bool) -> Result<(), CmdError> {
    let registry = read_registry().await?;
    let mut deliveries: Vec<Delivery> = read_deliveries(&registry)
        .into_iter()
        .filter(|delivery| delivery.state.open())
        .filter(|delivery| matches(delivery, product.as_deref()))
        .collect();
    deliveries.sort_by(|left, right| left.delivered_at.cmp(&right.delivered_at));
    if json_output {
        return print_json(&serde_json::to_value(&deliveries)?);
    }
    if deliveries.is_empty() {
        println!("(nothing is waiting for proof)");
        return Ok(());
    }
    println!(
        "{:<24} {:<10} {:<12} {:<22} {}",
        "PRODUCT", "REVISION", "STATE", "DELIVERED", "WHAT"
    );
    for delivery in &deliveries {
        println!(
            "{:<24} {:<10} {:<12} {:<22} {}",
            delivery.product,
            delivery.short_revision(),
            delivery.state.as_str(),
            delivery.delivered_at,
            delivery.summary.as_deref().unwrap_or("-")
        );
    }
    Ok(())
}

/// The passes and what they answered for.
pub(super) async fn status(product: Option<String>, json_output: bool) -> Result<(), CmdError> {
    let registry = read_registry().await?;
    let deliveries = read_deliveries(&registry);
    let mut passes: Vec<QualificationPass> = read_passes(&registry)
        .into_iter()
        .filter(|pass| {
            product
                .as_deref()
                .is_none_or(|product| pass.product == product)
        })
        .collect();
    passes.sort_by(|left, right| right.started_at.cmp(&left.started_at));
    if json_output {
        return print_json(&json!({
            "passes": serde_json::to_value(&passes)?,
            "deliveries": serde_json::to_value(&deliveries)?,
        }));
    }
    if passes.is_empty() {
        println!("(no qualification pass has run yet)");
        return Ok(());
    }
    for pass in &passes {
        println!(
            "{} {} {} ({} delivery(ies))",
            pass.id,
            pass.status,
            pass.started_at,
            pass.deliveries.len()
        );
        if let Some(reason) = &pass.reason {
            println!("  reason: {reason}");
        }
        for (platform, job) in &pass.jobs {
            println!("  {platform}: {job}");
        }
        for delivery in deliveries
            .iter()
            .filter(|delivery| pass.deliveries.contains(&delivery.id))
        {
            println!(
                "  {:<10} {:<10} {}",
                delivery.short_revision(),
                delivery.state.as_str(),
                delivery.task.as_deref().unwrap_or("-")
            );
        }
    }
    Ok(())
}

/// Failed deliveries that have not been handed back yet.
///
/// This is the list a task register reads: each row names the task the
/// revision answered and the sentence the pass gave, which is everything
/// needed to reopen the work where it was done.
pub(super) async fn failures(unreported: bool, json_output: bool) -> Result<(), CmdError> {
    let registry = read_registry().await?;
    let failed: Vec<Delivery> = read_deliveries(&registry)
        .into_iter()
        .filter(|delivery| delivery.state == DeliveryState::Failed)
        .filter(|delivery| !unreported || !delivery.reported)
        .collect();
    if json_output {
        return print_json(&serde_json::to_value(&failed)?);
    }
    if failed.is_empty() {
        println!("(no failed delivery is waiting to be handed back)");
        return Ok(());
    }
    for delivery in &failed {
        println!(
            "{} {} task={} reported={} — {}",
            delivery.product,
            delivery.short_revision(),
            delivery.task.as_deref().unwrap_or("-"),
            delivery.reported,
            delivery.reason.as_deref().unwrap_or("no reason recorded")
        );
    }
    Ok(())
}

fn matches(delivery: &Delivery, product: Option<&str>) -> bool {
    product.is_none_or(|product| delivery.product == product)
}
