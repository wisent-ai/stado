//! The delivery routine as `stado ci status` and Stado Desktop show it.

use chrono::Utc;
use serde_json::{json, Value};

use super::{plan, state};
use crate::cli::CmdError;

/// The plan, the step running and how long it has run, the refused step
/// with every refusal and its defects, and what each step last delivered.
pub fn status() -> Result<Value, CmdError> {
    let current = state::read()?;
    let running = current.running.as_ref().map(|running| {
        json!({
            "step": running.step,
            "revision": running.revision,
            "started_at": running.started_at,
            "age_seconds": (Utc::now() - running.started_at).num_seconds(),
        })
    });
    Ok(json!({
        "plan": plan::read()?,
        "plan_path": plan::path()?,
        "state_path": state::path()?,
        "running": running,
        "refused": current.refused,
        "delivered": current.delivered,
        "pushed": current.pushed,
        "last_pass": current.last_pass,
    }))
}

/// The routine's lines in `stado ci status`'s plain form.
pub fn print_status(document: &Value) {
    match document["plan"]["steps"].as_array() {
        None => println!(
            "delivery routine: no plan declared at {}",
            document["plan_path"]
        ),
        Some(steps) => {
            let names: Vec<String> = steps
                .iter()
                .map(|step| format!("{}:{}", step["product"], step["surface"]))
                .collect();
            println!("delivery routine plan: {}", names.join(" -> "));
        }
    }
    if let Some(running) = document["running"].as_object() {
        println!(
            "  running: {} at {} for {}s",
            running["step"], running["revision"], running["age_seconds"]
        );
    }
    if let Some(refused) = document["refused"].as_object() {
        println!(
            "  refused: {} at {} on {}, defects {}",
            refused["step"], refused["revision"], refused["at"], refused["defects"]
        );
    }
    for (step, delivered) in document["delivered"].as_object().into_iter().flatten() {
        println!(
            "  delivered: {step} at {} on {}",
            delivered["revision"], delivered["at"]
        );
    }
    for (checkout, pushed) in document["pushed"].as_object().into_iter().flatten() {
        match pushed["refusal"].as_str() {
            Some(refusal) => println!(
                "  push refused: {checkout} at {} on {}: {refusal}",
                pushed["revision"], pushed["at"]
            ),
            None => println!(
                "  pushed: {checkout} at {} on {}",
                pushed["revision"], pushed["at"]
            ),
        }
    }
    if let Some(last) = document["last_pass"].as_object() {
        println!("  last pass {}: {}", last["ended_at"], last["said"]);
    }
}
