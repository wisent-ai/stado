//! The GCP half of `billing show`: the latest month of the BigQuery
//! billing export plus the 7-day credit burn rate.

use serde_json::Value;

use crate::cli::reporting::{field, NOT_REPORTED};

pub(super) fn print_gcp(section: &Value) {
    if section.get("status").and_then(Value::as_str) != Some("ok") {
        println!(
            "GCP: {} — {}",
            field(&section["status"]),
            field(&section["detail"])
        );
        return;
    }
    let month = section
        .get("monthly")
        .and_then(Value::as_array)
        .and_then(|rows| rows.last());
    match month {
        Some(month) => println!(
            "GCP {}: gross={} credits={} net={} {}",
            field(&month["month"]),
            field(&month["gross"]),
            field(&month["credits"]),
            field(&month["net"]),
            field(&month["currency"]),
        ),
        None => println!("GCP monthly billing: {NOT_REPORTED}"),
    }
    println!(
        "GCP credit burn: {}/day",
        field(&section["avg_daily_credit_applied_7d"])
    );
}
