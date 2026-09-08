//! How a comparison renders: the JSON shape, the table, and the list of
//! every divergent object.

use crate::cli::storage::*;

pub(in crate::cli::storage) fn diff_json(diff: &PrefixDiff) -> Value {
    json!({
        "prefix": diff.prefix,
        "source_objects": diff.source_objects,
        "destination_objects": diff.destination_objects,
        "missing_at_destination": diff.missing,
        "only_at_destination": diff.extra,
        "metadata_mismatches": diff
            .metadata_gaps
            .iter()
            .map(|(name, keys)| json!({"name": name, "keys": keys}))
            .collect::<Vec<Value>>(),
        "body_mismatches": diff.body_mismatches,
        "body_read_errors": diff
            .body_errors
            .iter()
            .map(|(name, error)| json!({"name": name, "error": error}))
            .collect::<Vec<Value>>(),
        "source_error": diff.source_error,
        "destination_error": diff.destination_error,
        "diverged": diff.diverged(),
    })
}

pub(in crate::cli::storage) fn print_diff_table(diffs: &[PrefixDiff]) {
    let rows: Vec<Vec<String>> = diffs
        .iter()
        .map(|diff| {
            vec![
                diff.prefix.clone(),
                render_count(diff.source_objects),
                render_count(diff.destination_objects),
                diff.missing.len().to_string(),
                diff.extra.len().to_string(),
                diff.metadata_gaps.len().to_string(),
                (diff.body_mismatches.len() + diff.body_errors.len()).to_string(),
                diff.status(),
            ]
        })
        .collect();
    print_table(
        &[
            "PREFIX",
            "AT SOURCE",
            "AT DESTINATION",
            "MISSING",
            "EXTRA",
            "META-DIFF",
            "BODY-DIFF",
            "STATUS",
        ],
        &rows,
    );
}

/// Name every divergent object. An operator mid-cutover needs the list, not
/// a tally, and the list is what tells them whether the gap is churn or a
/// dropped prefix.
pub(in crate::cli::storage) fn print_diff_detail(diffs: &[PrefixDiff]) {
    for diff in diffs.iter().filter(|diff| diff.diverged()) {
        println!("\n{}:", diff.prefix);
        if let Some(error) = &diff.source_error {
            println!("  source could not be listed: {error}");
        }
        if let Some(error) = &diff.destination_error {
            println!("  destination could not be listed: {error}");
        }
        for name in &diff.missing {
            println!("  missing at destination: {name}");
        }
        for name in &diff.extra {
            println!("  only at destination: {name}");
        }
        for (name, keys) in &diff.metadata_gaps {
            println!("  metadata did not land on {name}: {}", keys.join(", "));
        }
        for name in &diff.body_mismatches {
            println!("  body differs: {name}");
        }
        for (name, error) in &diff.body_errors {
            println!("  body unreadable for {name}: {error}");
        }
    }
}
