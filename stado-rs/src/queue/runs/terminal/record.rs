//! CAS the settled outcome of one member job into its run manifest entry.

use chrono::Utc;
use serde_json::Value;

use crate::queue::storage::JobStorage;
use crate::queue::StorageError;

use super::super::prefixes::{RUN_PREFIX, TERMINAL_PREFIXES};
use super::projection::terminal_job_matches_entry;

/// Retain an exact terminal job before its source transition can be retired.
///
/// Jobs predating durable submission manifests have no submission identity
/// and are left on their legacy lifecycle path. A linked v3 job must still
/// have its manifest: the terminal destination is already durable when this
/// runs, so refusing an absent manifest preserves the settled result and its
/// source fence for recovery rather than losing the only chance to retain it.
pub async fn record_terminal_outcome(
    store: &JobStorage,
    job: &crate::models::Job,
    prefix: &str,
) -> Result<(), StorageError> {
    if !TERMINAL_PREFIXES.contains(&prefix) {
        return Err(StorageError::Other(format!(
            "{prefix} is not a terminal job prefix"
        )));
    }
    let Some(index) = job.submission_command_index else {
        return Ok(());
    };
    if job.run_id.is_empty() || job.submission_request_digest.is_empty() {
        return Ok(());
    }
    record_terminal_outcome_inner(
        store,
        &job.run_id,
        index,
        job,
        prefix,
        MissingManifest::Refuse,
    )
    .await
}

#[derive(Clone, Copy)]
enum MissingManifest {
    Allow,
    Refuse,
}

/// Retain one terminal job against the durable manifest entry that names it.
///
/// Reaping supplies the manifest identity explicitly so runs migrated after a
/// legacy terminal transition can retain their exact outcome. Such a terminal
/// job may omit all three submission-linkage fields, but a partial or
/// conflicting linkage is still rejected.
///
/// A reaper that already read the manifest supplies its identity explicitly.
/// If another owner removes that manifest before retention, its deletion wins
/// and this stale reaper stops without recreating it.
pub(crate) async fn record_terminal_outcome_for_entry(
    store: &JobStorage,
    run_id: &str,
    index: usize,
    job: &crate::models::Job,
    prefix: &str,
) -> Result<(), StorageError> {
    record_terminal_outcome_inner(store, run_id, index, job, prefix, MissingManifest::Allow).await
}

async fn record_terminal_outcome_inner(
    store: &JobStorage,
    run_id: &str,
    index: usize,
    job: &crate::models::Job,
    prefix: &str,
    missing_manifest: MissingManifest,
) -> Result<(), StorageError> {
    if !TERMINAL_PREFIXES.contains(&prefix) {
        return Err(StorageError::Other(format!(
            "{prefix} is not a terminal job prefix"
        )));
    }
    crate::queue::submit::validate_run_id(run_id)
        .map_err(|error| StorageError::Other(error.to_string()))?;
    let path = format!("{RUN_PREFIX}/{run_id}.json");
    match crate::queue::submit::migrate_v2_run_manifest(store, run_id).await {
        Ok(_) => {}
        Err(crate::queue::submit::SubmitError::Storage(StorageError::NotFound(missing))) => {
            return match missing_manifest {
                MissingManifest::Allow => Ok(()),
                MissingManifest::Refuse => Err(StorageError::NotFound(missing)),
            };
        }
        Err(error) => return Err(StorageError::Other(error.to_string())),
    }
    for _ in 0..16 {
        let Some(versioned) = store.read_text_versioned(&path).await? else {
            return match missing_manifest {
                MissingManifest::Allow => Ok(()),
                MissingManifest::Refuse => Err(StorageError::NotFound(path)),
            };
        };
        let mut manifest: Value = serde_json::from_str(&versioned.content)?;
        crate::queue::submit::validate_stored_run_manifest(&manifest, run_id)
            .map_err(|error| StorageError::Other(error.to_string()))?;
        if manifest.get("schema").and_then(Value::as_str) != Some("stado.run-submission.v3") {
            return Err(StorageError::Other(format!(
                "durable run manifest {run_id} does not match terminal job {}",
                job.job_id
            )));
        }
        let entry = manifest
            .get_mut("entries")
            .and_then(Value::as_array_mut)
            .and_then(|entries| entries.get_mut(index))
            .and_then(Value::as_object_mut)
            .ok_or_else(|| {
                StorageError::Other(format!(
                    "durable run manifest {run_id} has no entry {index}"
                ))
            })?;
        if entry.get("job_id").and_then(Value::as_str) != Some(job.job_id.as_str()) {
            return Err(StorageError::Other(format!(
                "durable run manifest {run_id} maps entry {index} to a different job"
            )));
        }
        let planned: crate::models::Job =
            serde_json::from_value(entry.get("planned_job").cloned().ok_or_else(|| {
                StorageError::Other("durable run entry has no planned job".into())
            })?)?;
        if !terminal_job_matches_entry(job, &planned, run_id, index) {
            return Err(StorageError::Other(format!(
                "terminal job {} does not match its immutable run projection",
                job.job_id
            )));
        }
        if entry.get("state").and_then(Value::as_str) == Some("reaped") {
            return Ok(());
        }
        if let Some(existing) = entry.get("outcome") {
            let existing_prefix = existing.get("prefix").and_then(Value::as_str);
            let existing_job = existing.get("job");
            if existing_prefix == Some(prefix)
                && existing_job == Some(&serde_json::to_value(job).expect("Job serialization"))
            {
                return Ok(());
            }
            return Err(StorageError::Other(format!(
                "terminal outcome for job {} changed",
                job.job_id
            )));
        }
        entry.insert("state".into(), Value::from("terminal"));
        entry.remove("owner");
        entry.remove("lease_expires_at");
        entry.insert(
            "outcome".into(),
            serde_json::json!({
                "prefix": prefix,
                "recorded_at": Utc::now().to_rfc3339(),
                "job": job,
            }),
        );
        match store
            .compare_and_swap_text(
                &path,
                &versioned.version,
                &serde_json::to_string_pretty(&manifest)?,
            )
            .await
        {
            Ok(_) => return Ok(()),
            Err(StorageError::StorageConflict(_)) => continue,
            Err(error) => return Err(error),
        }
    }
    Err(StorageError::StorageConflict(format!(
        "run manifest {run_id} remained contended while recording job {}",
        job.job_id
    )))
}
