//! Re-submitting the gaps a walk found, and the load-verify-retry loop.

use serde_json::Value;

use super::state::{state_load, state_save, state_slot};
use super::walk::verify;
use super::CoverageReport;
use crate::config;
use crate::coverage::{CoverageError, Universe};
use crate::queue::submit::{stable_run_id, submit_batch, SubmitOptions};
use crate::queue::JobStorage;

/// Retry every gap through its own durable one-entry run because verify_command
/// is entry-specific. The universe/group/attempt tuple is retained in coverage
/// state, so a crash before the attempt checkpoint retries the same manifest.
pub async fn retry_gaps(
    universe: &dyn Universe,
    report: &CoverageReport,
    state: &mut Value,
    store: &JobStorage,
    batch_label: &str,
    log: Option<&dyn Fn(String)>,
) -> Result<usize, CoverageError> {
    if report.gaps.is_empty() {
        return Ok(0);
    }
    let generation = report
        .gaps
        .iter()
        .map(|gap| {
            let attempts = state_slot(state, &gap.group_key)
                .get("attempts")
                .and_then(Value::as_i64)
                .unwrap_or(0);
            format!("{}:{attempts}", gap.group_key)
        })
        .collect::<Vec<_>>()
        .join("\0");
    let batch_id = if batch_label.is_empty() {
        stable_run_id(
            "coverage-batch",
            &format!("{}\0{generation}", universe.id()),
        )
    } else {
        batch_label.to_string()
    };
    let base = universe.submit_options();
    let mut submitted = 0usize;
    for gap in &report.gaps {
        let verify_command = gap
            .extra
            .get("verify_command")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let attempts = state_slot(state, &gap.group_key)
            .get("attempts")
            .and_then(Value::as_i64)
            .unwrap_or(0);
        let token = format!(
            "{}\0{}\0{}\0{}",
            universe.id(),
            gap.group_key,
            attempts,
            gap.command
        );
        let run_id = stable_run_id("coverage", &token);
        let options = SubmitOptions {
            batch_id: batch_id.clone(),
            run_id,
            bucket: config::bucket().to_string(),
            verify_command,
            ..base.clone()
        };
        submit_batch(std::slice::from_ref(&gap.command), &options).await?;
        submitted += 1;
    }
    let now = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();
    for entry in &report.gaps {
        let slot = state_slot(state, &entry.group_key);
        let attempts = slot.get("attempts").and_then(Value::as_i64).unwrap_or(0);
        slot.insert("attempts".into(), Value::from(attempts + 1));
        slot.insert("last_batch_id".into(), Value::from(batch_id.as_str()));
        slot.insert("last_submitted_at".into(), Value::from(now.as_str()));
    }
    state_save(store, universe.id(), state).await?;
    if let Some(log) = log {
        log(format!(
            "[{}] submitted {submitted}/{} in batch {batch_id}",
            universe.id(),
            report.gaps.len()
        ));
    }
    Ok(submitted)
}

/// Python `verify_and_retry`: load state -> verify -> if execute, retry
/// gaps. The store comes from `config::bucket()` like Python
/// `JobStorage(BUCKET)`.
pub async fn verify_and_retry(
    universe: &dyn Universe,
    execute: bool,
    threads: usize,
    log: Option<&dyn Fn(String)>,
) -> Result<CoverageReport, CoverageError> {
    let store = JobStorage::with_bucket(config::bucket()).await?;
    verify_and_retry_with_store(universe, &store, execute, threads, log).await
}

/// [`verify_and_retry`] with an explicit store (offline/test seam; the
/// Python equivalent of constructing `JobStorage` yourself).
pub async fn verify_and_retry_with_store(
    universe: &dyn Universe,
    store: &JobStorage,
    execute: bool,
    threads: usize,
    log: Option<&dyn Fn(String)>,
) -> Result<CoverageReport, CoverageError> {
    let mut state = state_load(store, universe.id()).await?;
    let report = verify(universe, threads, &state, log).await?;
    if execute {
        retry_gaps(universe, &report, &mut state, store, "", log).await?;
    }
    Ok(report)
}
