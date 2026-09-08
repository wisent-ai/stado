//! The parallel walk that classifies every universe entry against state.

use futures::StreamExt;
use serde_json::Value;

use super::CoverageReport;
use crate::config;
use crate::coverage::{CoverageError, Universe, UniverseEntry, PRESENT};

/// Python `verify`: walk the universe in parallel (thread pool ->
/// `buffer_unordered(threads)`), classify each entry against `state`,
/// build a report. Progress is logged every
/// `COVERAGE_PROGRESS_LOG_EVERY` completed entries.
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

    let mut present_n = 0usize;
    let mut gaps: Vec<UniverseEntry> = Vec::new();
    let mut unfixable: Vec<(String, String)> = Vec::new();
    for (index, result) in results.into_iter().enumerate() {
        let (entry, status) = result?;
        let done = index + 1;
        if let Some(log) = log {
            if done as i64 % config::COVERAGE_PROGRESS_LOG_EVERY == 0 {
                log(format!("[{}] {done}/{total}", universe.id()));
            }
        }
        if status == PRESENT {
            present_n += 1;
            continue;
        }
        let slot = state.get(&entry.group_key);
        let attempts = slot
            .and_then(|s| s.get("attempts"))
            .and_then(Value::as_i64)
            .unwrap_or(0);
        if attempts >= config::COVERAGE_ATTEMPT_CAP {
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
