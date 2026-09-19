//! What the release pipeline's own run records say about a version.

use std::os::unix::fs::MetadataExt;
use std::path::Path;

use serde_json::Value;

use crate::providers::local::disk_cleanup::release_store::{RunRetentionEvidence, RUNS_PREFIX};
use crate::providers::local::disk_cleanup::JanitorError;

/// Publication evidence from every namespace on the local store.
///
/// Missing from the active-run set does not mean a publisher finished. The
/// tag workflow publishes outside the queue: on 2026-09-06 the janitor removed
/// Stado 0.16.29 repeatedly while that workflow was uploading it. Reclaim
/// requires a completed run for the same source, not merely an absent pin.
/// Unknown and failed publications remain owned by their publisher.
pub(in crate::providers::local::disk_cleanup::release_store) fn run_retention_evidence(
    ecosystem: &Path,
    min_age_seconds: i64,
    now_epoch: i64,
) -> Result<RunRetentionEvidence, JanitorError> {
    let mut evidence = RunRetentionEvidence::default();
    let mut directories = Vec::new();
    for namespace in std::fs::read_dir(ecosystem)? {
        let namespace = namespace?;
        if namespace.file_type()?.is_dir() {
            directories.push((namespace.path().join(RUNS_PREFIX), 0usize));
        }
    }
    while let Some((directory, depth)) = directories.pop() {
        let entries = match std::fs::read_dir(&directory) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => {
                return Err(JanitorError::os(&format!(
                    "list release runs {}: {error}",
                    directory.display()
                )));
            }
        };
        for entry in entries {
            let entry = entry?;
            let kind = entry.file_type()?;
            if kind.is_dir() && depth < 2 {
                directories.push((entry.path(), depth + 1));
                continue;
            }
            if !kind.is_file() || entry.file_name() != "run.json" {
                continue;
            }
            let record = entry.path();
            let text = std::fs::read_to_string(&record).map_err(|error| {
                JanitorError::os(&format!("read release run {}: {error}", record.display()))
            })?;
            let run = serde_json::from_str::<Value>(&text).map_err(|error| {
                JanitorError::os(&format!("parse release run {}: {error}", record.display()))
            })?;
            let (Some(product), Some(version)) = (
                run.get("product").and_then(Value::as_str),
                run.get("version").and_then(Value::as_str),
            ) else {
                continue;
            };
            let state = run
                .get("state")
                .and_then(Value::as_str)
                .and_then(crate::release_pipeline::ReleaseRunState::named);
            // A superseded run published nothing and is not retained as
            // finished work, so it is pinned like a run still in flight.
            let terminal = state.as_ref().is_some_and(|state| {
                state.finished() && *state != crate::release_pipeline::ReleaseRunState::Superseded
            });
            let young = std::fs::metadata(&record)
                .map(|meta| now_epoch - meta.mtime() < min_age_seconds)
                .unwrap_or(true);
            if !terminal || young {
                evidence
                    .pinned
                    .entry(product.to_string())
                    .or_default()
                    .insert(version.to_string());
            } else if state.is_some_and(|state| state.published()) {
                if let Some(source) = run.get("source_commit").and_then(Value::as_str) {
                    evidence
                        .finished
                        .entry(product.to_string())
                        .or_default()
                        .entry(version.to_string())
                        .or_default()
                        .insert(source.to_string());
                }
            }
        }
    }
    Ok(evidence)
}
