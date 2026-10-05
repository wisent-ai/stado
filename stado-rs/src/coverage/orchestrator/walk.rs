//! The parallel walk that classifies every universe entry against state.

use futures::StreamExt;
use serde_json::Value;

use super::CoverageReport;
use crate::coverage::{CoverageError, Universe, UniverseEntry, PRESENT};

/// Python `verify`: walk the universe in parallel (thread pool ->
/// `buffer_unordered(threads)`), classify each entry against `state`,
/// build a report.
pub async fn verify(
    universe: &dyn Universe,
    threads: usize,
    state: &Value,
    log: Option<&dyn Fn(String)>,
) -> Result<CoverageReport, CoverageError> {
    let entries = universe.iter_entries();
    let total = entries.len();
    let verifier = universe.verifier();
    let results: Vec<Result<(UniverseEntry, String), CoverageError>> =
        futures::stream::iter(entries)
            .map(|entry| {
                let verifier = &verifier;
                async move {
                    let status = verifier.check(&entry.expected_uri).await?;
                    Ok((entry, status))
                }
            })
            .buffer_unordered(threads)
            .collect()
            .await;
    if let Some(log) = log {
        log(format!("[{}] {total}/{total} verified", universe.id()));
    }
    let mut present_n = 0usize;
    let mut gaps: Vec<UniverseEntry> = Vec::new();
    let mut unfixable: Vec<(String, String)> = Vec::new();
    for result in results {
        let (entry, status) = result?;
        if status == PRESENT {
            present_n += 1;
            continue;
        }
        // A gap whose last two failed jobs gave the same error is not
        // submitted a third time: nothing between them changed the outcome.
        let slot = state.get(&entry.group_key);
        if slot
            .and_then(|s| s.get("repeated_error"))
            .and_then(Value::as_bool)
            == Some(true)
        {
            let last_err = slot
                .and_then(|s| s.get("last_error"))
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            unfixable.push((entry.group_key.clone(), last_err));
            continue;
        }
        gaps.push(entry);
    }
    Ok(CoverageReport {
        universe_id: universe.id().to_string(),
        total_entries: total,
        present: present_n,
        missing: total - present_n,
        unfixable,
        gaps,
        opaque: Vec::new(),
    })
}
