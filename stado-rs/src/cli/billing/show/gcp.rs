//! The GCP half of `billing show`: the latest month of the BigQuery
//! billing export plus the 7-day credit burn rate.

use serde_json::Value;

use crate::cli::billing::format::text;

pub(super) fn print_gcp(section: &Value) {
    if section.get("status").and_then(Value::as_str) != Some("ok") {
        println!(
            "GCP: {} — {}",
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
    let month = section
        .get("monthly")
        .and_then(Value::as_array)
        .and_then(|rows| rows.last());
    if let Some(month) = month {
        println!(
            "GCP {}: gross={} credits={} net={} {}",
            text(month.get("month")),
            text(month.get("gross")),
            text(month.get("credits")),
            text(month.get("net")),
            text(month.get("currency")),
        );
    }
    println!(
        "GCP credit burn: {}/day",
        text(section.get("avg_daily_credit_applied_7d"))
    );
}
