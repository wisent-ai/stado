//! The coverage lines of the human-readable report.
//!
//! The order is the order an operator acts in: the rule and the volume's
//! reading, the verdict and its sentence, the janitor's last pass, then the
//! paths nothing covers, largest first.

use serde_json::Value;

/// Bytes as an operator reads them, through the same helper the rest of this
/// capability uses for disk figures.
pub fn gib(bytes: i64) -> String {
    format!(
        "{:.1} GiB",
        crate::deploy::host_disk::gib_from_blocks((bytes / 1024) as f64)
    )
}

/// Print the rule line, the verdict, the janitor sentence and every
/// uncovered path.
pub fn print_coverage(coverage: &Value) {
    let rule = &coverage["rule"];
    match rule["used_percent"].as_f64() {
        Some(used) => println!(
            "rule: delete everything the fleet put here at {}% used; volume {used:.1}% used ({})",
            rule["full_percent"],
            if rule["triggered"].as_bool() == Some(true) {
                "triggered"
            } else {
                "not triggered"
            }
        ),
        None => println!(
            "rule: delete everything the fleet put here at {}% used; volume unreadable",
            rule["full_percent"]
        ),
    }
    println!(
        "disk: {} — {}",
        coverage
            .get("verdict")
            .and_then(Value::as_str)
            .unwrap_or("unknown"),
        coverage
            .get("detail")
            .and_then(Value::as_str)
            .unwrap_or("no coverage detail"),
    );
    if let Some(janitor) = coverage.get("janitor") {
        println!(
            "janitor: {} — {}",
            janitor
                .get("outcome")
                .and_then(Value::as_str)
                .unwrap_or("never_run"),
            janitor
                .get("detail")
                .and_then(Value::as_str)
                .unwrap_or("no janitor detail"),
        );
        if let Some(pass) = janitor.get("report").filter(|pass| pass.is_object()) {
            if let Some(cleaners) = pass.get("cleaners").and_then(Value::as_object) {
                for (name, result) in cleaners {
                    println!(
                        "cleaner {name}: scanned {}, eligible {}, deleted {}; retained {}",
                        result["scanned_items"],
                        result["eligible_items"],
                        result["deleted_items"],
                        result["skipped"]
                    );
                }
            }
            if let Some(errors) = pass.get("errors").and_then(Value::as_array) {
                for error in errors {
                    println!("cleanup error: {error}");
                }
            }
        }
    }
    let empty = Vec::new();
    // The first word of each row is the mechanism, not a verdict about the
    // path: the cleaner whose area holds it, or `uncovered` when nothing in
    // the product looks there — the user's data.
    for row in coverage
        .get("uncovered")
        .and_then(Value::as_array)
        .unwrap_or(&empty)
    {
        let label = row
            .get("mechanism")
            .and_then(Value::as_str)
            .unwrap_or("uncovered");
        println!(
            "{label}\t{}\t{}",
            gib(row.get("bytes").and_then(Value::as_i64).unwrap_or_default()),
            row.get("path").and_then(Value::as_str).unwrap_or("unknown"),
        );
        if row["exclusive_of_measured_children"].as_bool() == Some(true) {
            println!("  size excludes the measured child directories listed separately");
        }
    }
}
