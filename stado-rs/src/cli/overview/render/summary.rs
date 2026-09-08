//! The two lines that open the report: which snapshot this is, and where the
//! queue stands.
//!
//! The stamp is printed before anything else on purpose — every number below
//! it is only as fresh as that line says.

use serde_json::Value;

pub(super) fn print_header(document: &Value) {
    println!("STADO OVERVIEW");
    println!(
        "generated: {}",
        document
            .get("generated_at")
            .and_then(Value::as_str)
            .unwrap_or("unknown")
    );
}

pub(super) fn print_jobs(document: &Value) {
    let jobs = &document["jobs"];
    println!(
        "jobs: {} running | {} queued | {} completed | {} uploaded | {} failed",
        jobs["running"], jobs["queue"], jobs["completed"], jobs["uploaded"], jobs["failed"]
    );
}
