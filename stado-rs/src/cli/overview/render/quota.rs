//! The quota section: one row per provider and accelerator, and an explicit
//! line when a provider is visible but has no live quota at all.
//!
//! A provider that answers with nothing is not the same as a provider that
//! could not be reached, so the two cases print different words.

use serde_json::Value;

use crate::cli::reporting::field;

pub(super) fn print_quota(document: &Value) {
    println!("quota:");
    let quota = &document["quota"];
    if quota.get("status").and_then(Value::as_str) == Some("error") {
        match quota.get("detail").and_then(Value::as_str) {
            Some(detail) => println!("  unavailable: {detail}"),
            None => println!("  unavailable, and the report gives no reason: {quota}"),
        }
    } else if let Some(providers) = quota.as_object() {
        for (provider, rows) in providers {
            let Some(rows) = rows.as_object() else {
                println!("  {provider}: unavailable");
                continue;
            };
            if rows.is_empty() {
                println!("  {provider}: no live quota visible");
            }
            for (accel, row) in rows {
                println!(
                    "  {provider}/{accel}: total={} used={} reserved={} available={}",
                    field(&row["total"]), field(&row["used"]), field(&row["reserved"]), field(&row["available"])
                );
            }
        }
    } else {
        println!("  unavailable: malformed quota response");
    }
}
