//! The human rendering of a published `billing_health/credits.json`, shared
//! by `billing show` and `billing refresh`.
//!
//! There is no per-provider branch here beyond the adapter the billing
//! capability declares for each variant, so a new provider in the catalog
//! prints through this same loop. `gcp` and `azure` hold the two section
//! renderers it dispatches to.

mod azure;
mod gcp;

use serde_json::Value;

use azure::print_azure;
use gcp::print_gcp;

pub(super) fn print_human(document: &Value) {
    if document.get("status").and_then(Value::as_str) == Some("unavailable") {
        println!(
            "billing unavailable: {}",
            document
                .get("detail")
                .and_then(Value::as_str)
                .unwrap_or("unknown error")
        );
        return;
    }
    println!(
        "billing reported: {}",
        document
            .get("reported_at")
            .and_then(Value::as_str)
            .unwrap_or("unknown")
    );
    if let Some(capability) = crate::capabilities::get("billing") {
        for variant in capability.variants {
            let section = &document[variant.id];
            match variant.adapter {
                crate::capabilities::RuntimeAdapter::Billing(
                    crate::capabilities::BillingAdapter::Gcp,
                ) => print_gcp(section),
                crate::capabilities::RuntimeAdapter::Billing(
                    crate::capabilities::BillingAdapter::Azure,
                ) => print_azure(section),
                _ => println!("{}: unsupported billing adapter", variant.id),
            }
        }
    }
}
