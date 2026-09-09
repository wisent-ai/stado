//! The coverage lines of the human-readable report.
//!
//! The order is the order an operator acts in: how much free space there is
//! against what the registry declares, the verdict and its sentence, the
//! janitor's last pass beside the distance still to go, then the paths nothing
//! covers, largest first. Before this, the report printed a free-byte figure
//! and a capacity percentage and left every watermark it had already read
//! inside the JSON.

use serde_json::Value;

/// Bytes as an operator reads them, through the same helper the rest of this
/// capability uses for disk figures.
pub fn gib(bytes: i64) -> String {
    format!(
        "{:.1} GiB",
        crate::deploy::host_disk::gib_from_blocks((bytes / 1024) as f64)
    )
}

/// Print the free-space line, the verdict, the janitor sentence and every
/// uncovered path.
pub fn print_coverage(coverage: &Value, free_space: &Value) {
    let available = free_space.get("available_bytes").and_then(Value::as_i64);
    let low = free_space
        .get("low_watermark_bytes")
        .and_then(Value::as_i64);
    let target = free_space
        .get("target_watermark_bytes")
        .and_then(Value::as_i64);
    match (available, low, target) {
        (Some(free), Some(low), Some(target)) => println!(
            "free: {}, low watermark {}, target {}",
            gib(free),
            gib(low),
            gib(target)
        ),
        (Some(free), _, _) => println!("free: {}, no watermark declared", gib(free)),
        _ => println!("free: unreadable"),
    }
    println!(
        "pressure: {} — {}",
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
    }
    let empty = Vec::new();
    for row in coverage
        .get("uncovered")
        .and_then(Value::as_array)
        .unwrap_or(&empty)
    {
        println!(
            "uncovered\t{}\t{}",
            gib(row.get("bytes").and_then(Value::as_i64).unwrap_or_default()),
            row.get("path").and_then(Value::as_str).unwrap_or("unknown"),
        );
    }
}
