//! Per-prefix object counts across the canonical prefix set.

use crate::cli::storage::*;

/// Per-prefix object counts across [`CANONICAL_PREFIXES`].
///
/// This is the fast operator answer during an outage, and the one place
/// where a listing failure must NOT render as an empty prefix: a store
/// that cannot be listed reports `unreachable` and the command exits
/// non-zero, so "the queue drained" can never be confused with "the queue
/// is behind a 403".
pub(in crate::cli::storage) async fn ls_canonical(
    store: &JobStorage,
    as_json: bool,
) -> Result<(), CmdError> {
    let backend = store.backend();
    let counted: Vec<(&str, Result<usize, String>)> = futures::stream::iter(CANONICAL_PREFIXES)
        .map(|prefix| async move {
            let outcome = backend
                .list_blobs_with_meta(prefix)
                .await
                .map(|blobs| blobs.len())
                .map_err(|err| err.to_string());
            (*prefix, outcome)
        })
        .buffered(copy::DEFAULT_CONCURRENCY)
        .collect()
        .await;

    let unreachable: Vec<&str> = counted
        .iter()
        .filter(|(_, outcome)| outcome.is_err())
        .map(|(prefix, _)| *prefix)
        .collect();
    let total: usize = counted
        .iter()
        .filter_map(|(_, outcome)| outcome.as_ref().ok())
        .sum();

    if as_json {
        let rows: Vec<Value> = counted
            .iter()
            .map(|(prefix, outcome)| match outcome {
                Ok(count) => json!({"prefix": prefix, "objects": count, "status": "ok"}),
                Err(error) => json!({
                    "prefix": prefix,
                    "objects": Value::Null,
                    "status": "unreachable",
                    "error": error,
                }),
            })
            .collect();
        echo_json(&json!({
            "backend": store.backend_name(),
            "bucket": store.bucket_name(),
            "prefixes": rows,
            "objects": total,
            "unreachable": unreachable,
        }))?;
    } else {
        let rows: Vec<Vec<String>> = counted
            .iter()
            .map(|(prefix, outcome)| match outcome {
                Ok(count) => vec![(*prefix).to_string(), count.to_string(), "ok".to_string()],
                Err(error) => vec![
                    (*prefix).to_string(),
                    String::new(),
                    format!("UNREACHABLE: {error}"),
                ],
            })
            .collect();
        println!("{} ({})", store.bucket_name(), store.backend_name());
        print_table(&["PREFIX", "OBJECTS", "STATUS"], &rows);
        println!(
            "\n{total} object(s) across {} readable prefix(es).",
            counted.len() - unreachable.len()
        );
    }

    if unreachable.is_empty() {
        return Ok(());
    }
    Err(CmdError::click(format!(
        "{} prefix(es) could not be listed ({}); those counts are UNKNOWN, not zero",
        unreachable.len(),
        unreachable.join(", ")
    )))
}
