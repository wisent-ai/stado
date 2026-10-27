//! The coverage lines of the human-readable report.
//!
//! The order is the order an operator acts in: the rule and the volume's
//! reading, the verdict and its sentence, the janitor's last pass, then the
//! paths nothing covers, largest first.

use serde_json::Value;

/// Bytes as an operator reads them, through the same helper the rest of this
/// capability uses for disk figures (it takes 1 KiB blocks, `df -Pk`'s unit).
pub fn gib(bytes: i64) -> String {
    use crate::providers::local::host_memory::constants::KIB;
    format!("{:.1} GiB", crate::deploy::host_disk::gib_from_blocks((bytes / KIB) as f64))
}

/// Print the rule line, the verdict, the janitor sentence and every
/// uncovered path. A field the report does not carry is said as
/// [`crate::cli::reporting::NOT_REPORTED`], never as a word nobody measured.
pub fn print_coverage(coverage: &Value) {
    use crate::cli::reporting::field;
    let rule = &coverage["rule"];
    let triggered = match rule["triggered"].as_bool() {
        Some(true) => "triggered",
        Some(false) => "not triggered",
        None => "whether it triggered is not reported",
    };
    match rule["used_percent"].as_f64() {
        Some(used) => println!(
            "rule: delete everything the fleet put here at {}% used; volume {used:.1}% used ({triggered})",
            field(&rule["full_percent"]),
        ),
        None => println!(
            "rule: delete everything the fleet put here at {}% used; volume unreadable",
            field(&rule["full_percent"])
        ),
    }
    match coverage.get("detail").and_then(Value::as_str) {
        Some(detail) => println!("disk: {} — {detail}", field(&coverage["verdict"])),
        None => println!("disk: {}", field(&coverage["verdict"])),
    }
    if let Some(janitor) = coverage.get("janitor") {
        println!("janitor: {} — {}", field(&janitor["outcome"]), field(&janitor["detail"]));
        if let Some(pass) = janitor.get("report").filter(|pass| pass.is_object()) {
            if let Some(cleaners) = pass.get("cleaners").and_then(Value::as_object) {
                for (name, result) in cleaners {
                    println!(
                        "cleaner {name}: scanned {}, eligible {}, deleted {}; retained {}",
                        field(&result["scanned_items"]),
                        field(&result["eligible_items"]),
                        field(&result["deleted_items"]),
                        field(&result["skipped"])
                    );
                }
            }
            if let Some(errors) = pass.get("errors").and_then(Value::as_array) {
                for error in errors {
                    println!("cleanup error: {}", field(error));
                }
            }
        }
    }
    let Some(uncovered) = coverage.get("uncovered").and_then(Value::as_array) else {
        println!("uncovered paths: {}", crate::cli::reporting::NOT_REPORTED);
        return;
    };
    // The first word of each row is the mechanism, not a verdict about the
    // path: the cleaner whose area holds it, or `uncovered` for a row that
    // names none, which is how the report says nothing in the product looks
    // there — the user's data.
    for row in uncovered {
        let label = match row["mechanism"].as_str() {
            Some(mechanism) => mechanism,
            None => "uncovered",
        };
        let size = match row["bytes"].as_i64() {
            Some(bytes) => gib(bytes),
            None => "size not reported".to_string(),
        };
        match row["path"].as_str() {
            Some(path) => println!("{label}\t{size}\t{path}"),
            None => println!("{label}\t{size}\ta coverage row without a path: {row}"),
        }
        if row["exclusive_of_measured_children"].as_bool() == Some(true) {
            println!("  size excludes the measured child directories listed separately");
        }
    }
}
