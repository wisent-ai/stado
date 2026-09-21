//! The single fenced registry write that records a completion pass.

use serde_json::Value;

use crate::targets::{RegistryStore, BUILDS_KEY};

use super::RunOutcome;

/// Write every outcome of one pass in a single fenced generation.
///
/// A per-outcome compare-and-swap would have a pass that finished four
/// platforms lose three generations to itself. The write is the raw-document
/// surgical edit the rest of this module uses, and it replaces a platform's
/// run only when the job id still matches the one that was reconciled: a
/// concurrent `builds run` for the same platform is a NEWER job, and stamping
/// a finished job's outcome over it would lose the submission.
pub(super) async fn commit_run_outcomes(outcomes: &[RunOutcome]) -> Result<(), String> {
    let store = RegistryStore::open()
        .await
        .map_err(|exc| format!("registry store open failed: {exc}"))?;
    let versioned = store
        .read_versioned()
        .await
        .map_err(|exc| format!("registry read failed: {exc}"))?
        .ok_or_else(|| format!("no registry document at {}", store.location()))?;
    let mut document: Value = serde_json::from_str(&versioned.content)
        .map_err(|exc| format!("registry parse failed: {exc}"))?;
    let mut written = 0usize;
    if let Some(entries) = document.get_mut(BUILDS_KEY).and_then(Value::as_array_mut) {
        written = record_runs(entries, outcomes)?;
    }

    // The deliveries this build was submitted for settle in the same
    // generation as its runs: a verdict written in a second write could be
    // lost while the run it came from survives, and a delivery that reads
    // `qualifying` forever is a task nobody gets back.
    let passes = super::passes::settle_passes(&mut document, outcomes);
    if written == 0 && passes == 0 {
        return Ok(());
    }
    let payload = format!(
        "{}\n",
        serde_json::to_string_pretty(&document)
            .map_err(|exc| format!("registry serialize failed: {exc}"))?
    );
    store
        .compare_and_swap(&versioned.version, &payload)
        .await
        .map_err(|exc| {
            format!("recording {written} run(s) lost a concurrent registry write: {exc}")
        })?;
    Ok(())
}

/// Write each outcome onto its recipe's run for that platform, and answer
/// how many were written.
///
/// A platform's run is replaced only when the job id still matches the one
/// that was reconciled: a concurrent `builds run` for the same platform is a
/// NEWER job, and stamping a finished job's outcome over it would lose the
/// submission.
fn record_runs(entries: &mut [Value], outcomes: &[RunOutcome]) -> Result<usize, String> {
    let mut written = 0usize;
    for outcome in outcomes {
        let Some(entry) = entries.iter_mut().find(|entry| {
            entry.get("name").and_then(Value::as_str) == Some(outcome.recipe.as_str())
        }) else {
            continue; // recipe removed while its build finished
        };
        let Some(object) = entry.as_object_mut() else {
            continue;
        };
        let runs = object
            .entry("runs".to_string())
            .or_insert_with(|| Value::Object(serde_json::Map::new()))
            .as_object_mut()
            .ok_or_else(|| format!("build recipe {:?}: runs is not an object", outcome.recipe))?;
        let recorded = runs
            .get(&outcome.platform)
            .and_then(|run| run.get("job_id"))
            .and_then(Value::as_str);
        if recorded != Some(outcome.run.job_id.as_str()) {
            continue;
        }
        runs.insert(
            outcome.platform.clone(),
            serde_json::to_value(&outcome.run)
                .map_err(|exc| format!("run serialize failed: {exc}"))?,
        );
        written += 1;
    }
    Ok(written)
}
