//! The retirement pass itself: retain each terminal outcome, mark the run
//! reaped, then delete what that committed snapshot permits.

use chrono::Utc;
use serde_json::{Map, Value};

use crate::queue::runs::{
    list_runs, record_terminal_outcome_for_entry, run_status, ALL_PREFIXES, RUN_PREFIX,
    TERMINAL_PREFIXES,
};
use crate::queue::{JobStorage, StorageError};

use super::manifest::{
    has_complete_retained_outcomes, manifest_job_ids, py_truthy, CLEANUP_COMPLETED_AT, REAPED_AT,
};
use super::{ReapRefusal, ReapSummary};

mod residue;
mod sweep;

use residue::{cleanup_residue_job_ids, retained_run_has_residue};
use sweep::sweep_retained_run;

/// Reap all fully-terminal runs. Returns a summary.
///
/// `limit > 0` caps how many runs this tick touches — a fresh reap or a
/// resumed cleanup both count against it, so an interrupted backlog cannot
/// make one tick unbounded; 0 means no cap.
pub async fn reap_terminal_runs(
    store: &JobStorage,
    limit: i64,
) -> Result<ReapSummary, StorageError> {
    let mut summary = ReapSummary::default();
    let mut touched = 0;
    let cleanup_residue = cleanup_residue_job_ids(store).await?;
    for run_id in list_runs(store).await? {
        let path = format!("{RUN_PREFIX}/{run_id}.json");
        let Some(initial) = store.read_text_versioned(&path).await? else {
            continue;
        };
        let initial_manifest: Value = serde_json::from_str(&initial.content)?;
        if initial_manifest.get("schema").and_then(Value::as_str) != Some("stado.run-submission.v3")
        {
            summary.refused_runs.push(ReapRefusal {
                run_id,
                reason: "unsupported legacy manifest schema; retained without cleanup",
            });
            continue;
        }
        crate::queue::submit::validate_stored_run_manifest(&initial_manifest, &run_id)
            .map_err(|error| StorageError::Other(error.to_string()))?;
        if initial_manifest
            .get(CLEANUP_COMPLETED_AT)
            .is_some_and(py_truthy)
        {
            if !has_complete_retained_outcomes(&initial_manifest) {
                summary.refused_runs.push(ReapRefusal {
                    run_id,
                    reason: "cleanup marker lacks complete retained terminal outcomes; retained without cleanup",
                });
                continue;
            }
            let job_ids = manifest_job_ids(&initial_manifest, &run_id)?;
            if !retained_run_has_residue(&cleanup_residue, &job_ids) {
                continue;
            }
            summary.deleted_jobs += sweep_retained_run(store, &run_id, &job_ids).await?;
            touched += 1;
            if limit > 0 && touched >= limit {
                break;
            }
            continue;
        }
        if initial_manifest.get(REAPED_AT).is_some_and(py_truthy) {
            if !has_complete_retained_outcomes(&initial_manifest) {
                summary.refused_runs.push(ReapRefusal {
                    run_id,
                    reason: "reaped marker lacks complete retained terminal outcomes; retained without cleanup",
                });
                continue;
            }
            // Retained, but the deletion pass that follows retention did not
            // finish. Resume exactly that: no outcome is retained again and
            // `reaped_at` is not rewritten, so this run is not counted as a
            // second reap.
            let job_ids = manifest_job_ids(&initial_manifest, &run_id)?;
            summary.deleted_jobs += sweep_retained_run(store, &run_id, &job_ids).await?;
            touched += 1;
            if limit > 0 && touched >= limit {
                break;
            }
            continue;
        }
        summary.examined_runs += 1;
        let Some(status) = run_status(store, &run_id).await? else {
            continue;
        };
        if !status.all_terminal {
            continue;
        }

        let entries = initial_manifest
            .get("entries")
            .and_then(Value::as_array)
            .ok_or_else(|| {
                StorageError::Other(format!("run manifest {run_id} missing durable entries"))
            })?;
        for (index, entry) in entries.iter().enumerate() {
            if entry.get("outcome").is_some_and(Value::is_object) {
                continue;
            }
            let job_id = entry.get("job_id").and_then(Value::as_str).ok_or_else(|| {
                StorageError::Other(format!("run manifest {run_id} has an invalid entry"))
            })?;
            let mut found = None;
            for prefix in TERMINAL_PREFIXES {
                if let Some(job) = store.read_job(prefix, job_id).await? {
                    found = Some((prefix, job));
                    break;
                }
            }
            let Some((prefix, job)) = found else {
                return Err(StorageError::Other(format!(
                    "terminal job {job_id} disappeared before run {run_id} retained its outcome"
                )));
            };
            record_terminal_outcome_for_entry(store, &run_id, index, &job, prefix).await?;
        }

        let Some(versioned) = store.read_text_versioned(&path).await? else {
            continue;
        };
        let mut manifest: Value = serde_json::from_str(&versioned.content)?;
        crate::queue::submit::validate_stored_run_manifest(&manifest, &run_id)
            .map_err(|error| StorageError::Other(error.to_string()))?;
        let entries = manifest
            .get_mut("entries")
            .and_then(Value::as_array_mut)
            .ok_or_else(|| {
                StorageError::Other(format!("run manifest {run_id} missing durable entries"))
            })?;
        let mut job_ids = Vec::with_capacity(entries.len());
        let reaped_at = Utc::now().format("%Y-%m-%dT%H:%M:%S+00:00").to_string();
        for entry in entries {
            if !entry.get("outcome").is_some_and(Value::is_object) {
                return Err(StorageError::Other(format!(
                    "run manifest {run_id} has a terminal entry without retained outcome"
                )));
            }
            let job_id = entry
                .get("job_id")
                .and_then(Value::as_str)
                .ok_or_else(|| {
                    StorageError::Other(format!("run manifest {run_id} has an invalid entry"))
                })?
                .to_string();
            job_ids.push(job_id);
            let entry = entry
                .as_object_mut()
                .expect("validated durable entry object");
            entry.insert("state".into(), Value::from("reaped"));
            entry.insert(REAPED_AT.into(), Value::from(reaped_at.as_str()));
        }
        let manifest_object = manifest
            .as_object_mut()
            .expect("validated run manifest object");
        manifest_object.insert(REAPED_AT.into(), Value::from(reaped_at));
        let counts: Map<String, Value> = ALL_PREFIXES
            .iter()
            .map(|prefix| (prefix.to_string(), Value::from(status.counts[*prefix])))
            .collect();
        manifest_object.insert("final_counts".into(), Value::Object(counts));
        match store
            .compare_and_swap_text(
                &path,
                &versioned.version,
                &serde_json::to_string_pretty(&manifest)?,
            )
            .await
        {
            Ok(_) => {}
            Err(StorageError::StorageConflict(_) | StorageError::NotFound(_)) => continue,
            Err(error) => return Err(error),
        }
        // The manifest is the deletion fence. Another coordinator may retire
        // it after our successful CAS; in that case do not guess a terminal
        // state and, critically, do not delete any job blobs. A storage read
        // error still propagates, while an absent object ends only this run's
        // cleanup.
        let Some(retained) = store.read_text_versioned(&path).await? else {
            continue;
        };
        let retained: Value = serde_json::from_str(&retained.content)?;
        crate::queue::submit::validate_stored_run_manifest(&retained, &run_id)
            .map_err(|error| StorageError::Other(error.to_string()))?;

        summary.deleted_jobs += sweep_retained_run(store, &run_id, &job_ids).await?;
        summary.reaped_runs += 1;
        touched += 1;
        if limit > 0 && touched >= limit {
            break;
        }
    }
    Ok(summary)
}
