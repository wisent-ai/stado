//! Durable run manifests are the admission and recovery authority above jobs.
//!
//! One submission request owns one `runs/<run_id>.json` document. Its `entries`
//! are CAS-mutated through planned/claimed/enqueuing/accepted/terminal/reaped;
//! terminal entries retain the final job document so deleting lifecycle blobs
//! never turns an old request into new queue work.

use std::collections::BTreeMap;

use chrono::Utc;
use serde_json::{Map, Value};

use super::storage::JobStorage;
use super::StorageError;

/// Blob prefix holding run manifests.
pub const RUN_PREFIX: &str = "runs";
/// Prefixes a job can no longer leave.
pub const TERMINAL_PREFIXES: [&str; 4] = ["completed", "uploaded", "failed", "cancelled"];
/// Every prefix a member job can sit in (probe order).
pub const ALL_PREFIXES: [&str; 6] = [
    "queue",
    "running",
    "completed",
    "uploaded",
    "failed",
    "cancelled",
];

/// `run-<unix seconds>-<8 hex chars>` (Python `generate_run_id`).
pub fn generate_run_id() -> String {
    let hex = uuid::Uuid::new_v4().simple().to_string();
    format!("run-{}-{}", Utc::now().timestamp(), &hex[..8])
}

/// Auto-derive a readable name from the run's commands: shared module +
/// model + the distinct --task values (or a count if many).
pub fn derive_run_name(commands: &[String]) -> String {
    let mut modules: Vec<String> = Vec::new();
    let mut models: Vec<String> = Vec::new();
    let mut tasks: Vec<String> = Vec::new();
    for command in commands {
        let toks: Vec<&str> = command.split_whitespace().collect();
        for (i, tok) in toks.iter().enumerate() {
            let next = toks.get(i + 1).copied().unwrap_or("");
            match *tok {
                "-m" => {
                    let module = next.rsplit('.').next().unwrap_or("").to_string();
                    if !modules.contains(&module) {
                        modules.push(module);
                    }
                }
                "--model" => {
                    let model = next
                        .trim_matches(['\'', '"'])
                        .rsplit('/')
                        .next()
                        .unwrap_or("")
                        .to_string();
                    if !models.contains(&model) {
                        models.push(model);
                    }
                }
                "--task" => tasks.push(next.to_string()),
                _ => {}
            }
        }
    }
    let mut parts: Vec<String> = Vec::new();
    // Python uses sets; with exactly one element iteration order is moot.
    if modules.len() == 1 {
        parts.push(modules[0].clone());
    }
    if models.len() == 1 {
        parts.push(models[0].clone());
    }
    // dict.fromkeys: distinct, first-seen order.
    let mut uniq: Vec<&str> = Vec::new();
    for task in &tasks {
        if !uniq.contains(&task.as_str()) {
            uniq.push(task);
        }
    }
    if (1..=3).contains(&uniq.len()) {
        parts.push(uniq.join("+"));
    } else if !uniq.is_empty() {
        parts.push(format!("{}tasks", uniq.len()));
    }
    parts.push(format!("{}jobs", commands.len()));
    parts.join(":")
}


/// Read a run manifest; `None` when it does not exist.
pub async fn read_run(
    store: &JobStorage,
    run_id: &str,
) -> Result<Option<Map<String, Value>>, StorageError> {
    let Some(versioned) = store
        .read_text_versioned(&format!("{RUN_PREFIX}/{run_id}.json"))
        .await?
    else {
        return Ok(None);
    };
    let mut value: Value = serde_json::from_str(&versioned.content)?;
    if value.get("schema").and_then(Value::as_str) == Some("stado.run-submission.v2") {
        value = crate::queue::submit::migrate_v2_run_manifest(store, run_id)
            .await
            .map_err(|error| StorageError::Other(error.to_string()))?;
    }
    match value {
        Value::Object(map) => Ok(Some(map)),
        _ => Err(StorageError::Other(format!(
            "run manifest {run_id} is not a JSON object"
        ))),
    }
}

/// Which prefix currently holds this job_id, or None if absent. Terminal wins
/// over transitional duplicates left by a crash during a fenced move.
async fn job_state(store: &JobStorage, job_id: &str) -> Result<Option<&'static str>, StorageError> {
    for prefix in ["cancelled", "failed", "uploaded", "completed", "running", "queue"] {
        if store.read_job(prefix, job_id).await?.is_some() {
            return Ok(Some(prefix));
        }
    }
    Ok(None)
}

/// Retain an exact terminal job before its lifecycle blobs can be deleted.
/// Jobs predating durable submission manifests have no submission identity
/// and are left on their legacy lifecycle path; v3 jobs fail closed on any
/// manifest mismatch.
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
    crate::queue::submit::migrate_v2_run_manifest(store, &job.run_id)
        .await
        .map_err(|error| StorageError::Other(error.to_string()))?;
    let path = format!("{RUN_PREFIX}/{}.json", job.run_id);
    for _ in 0..16 {
        let versioned = store.read_text_versioned(&path).await?.ok_or_else(|| {
            StorageError::Other(format!(
                "durable run manifest {} disappeared before terminal transition",
                job.run_id
            ))
        })?;
        let mut manifest: Value = serde_json::from_str(&versioned.content)?;
        if manifest.get("schema").and_then(Value::as_str)
            != Some("stado.run-submission.v3")
            || manifest.get("request_digest").and_then(Value::as_str)
                != Some(job.submission_request_digest.as_str())
        {
            return Err(StorageError::Other(format!(
                "durable run manifest {} does not match terminal job {}",
                job.run_id, job.job_id
            )));
        }
        let entry = manifest
            .get_mut("entries")
            .and_then(Value::as_array_mut)
            .and_then(|entries| entries.get_mut(index))
            .and_then(Value::as_object_mut)
            .ok_or_else(|| {
                StorageError::Other(format!(
                    "durable run manifest {} has no entry {index}",
                    job.run_id
                ))
            })?;
        if entry.get("job_id").and_then(Value::as_str) != Some(job.job_id.as_str()) {
            return Err(StorageError::Other(format!(
                "durable run manifest {} maps entry {index} to a different job",
                job.run_id
            )));
        }
        let planned: crate::models::Job = serde_json::from_value(
            entry
                .get("planned_job")
                .cloned()
                .ok_or_else(|| StorageError::Other("durable run entry has no planned job".into()))?,
        )?;
        if crate::queue::submit::immutable_job_projection(&planned)
            != crate::queue::submit::immutable_job_projection(job)
        {
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
        "run manifest {} remained contended while recording job {}",
        job.run_id, job.job_id
    )))
}

/// Per-state counts for a run, derived from its members' current prefixes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunStatus {
    pub run_id: String,
    pub submitter_app: String,
    pub n_jobs: i64,
    /// Count per prefix (all [`ALL_PREFIXES`] keys present).
    pub counts: BTreeMap<String, i64>,
    /// Member jobs present in no prefix.
    pub missing: i64,
    pub in_flight: i64,
    pub all_terminal: bool,
}

/// Derive per-state counts for a run; `None` if the manifest does not exist.
pub async fn run_status(
    store: &JobStorage,
    run_id: &str,
) -> Result<Option<RunStatus>, StorageError> {
    let Some(manifest) = read_run(store, run_id).await? else {
        return Ok(None);
    };
    let entries = manifest
        .get("entries")
        .and_then(Value::as_array)
        .ok_or_else(|| {
            StorageError::Other(format!(
                "run manifest {run_id} has no durable entries; resubmit or explicitly migrate it"
            ))
        })?;
    let mut counts: BTreeMap<String, i64> =
        ALL_PREFIXES.iter().map(|p| (p.to_string(), 0)).collect();
    let mut missing = 0;
    for entry in entries {
        let job_id = entry
            .get("job_id")
            .and_then(Value::as_str)
            .ok_or_else(|| StorageError::Other(format!("run manifest {run_id} has an invalid entry")))?;
        let retained = entry
            .get("outcome")
            .and_then(Value::as_object)
            .and_then(|outcome| outcome.get("prefix"))
            .and_then(Value::as_str);
        match retained {
            Some(prefix) if TERMINAL_PREFIXES.contains(&prefix) => {
                *counts.get_mut(prefix).expect("terminal prefix initialized") += 1;
            }
            Some(prefix) => {
                return Err(StorageError::Other(format!(
                    "run manifest {run_id} retained invalid outcome prefix {prefix}"
                )));
            }
            None => match job_state(store, job_id).await? {
                Some(prefix) => *counts.get_mut(prefix).expect("prefix initialized") += 1,
                None => missing += 1,
            },
        }
    }
    let terminal: i64 = TERMINAL_PREFIXES.iter().map(|p| counts[*p]).sum();
    let in_flight = counts["queue"] + counts["running"];
    Ok(Some(RunStatus {
        run_id: run_id.to_string(),
        submitter_app: manifest
            .get("submitter_app")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        n_jobs: entries.len() as i64,
        counts,
        missing,
        in_flight,
        all_terminal: in_flight == 0 && missing == 0 && terminal > 0,
    }))
}

/// Run ids of all manifests under `runs/`.
pub async fn list_runs(store: &JobStorage) -> Result<Vec<String>, StorageError> {
    let paths = store.list_paths(&format!("{RUN_PREFIX}/"), 0).await?;
    Ok(paths
        .iter()
        .filter_map(|p| p.rsplit('/').next())
        .filter(|name| name.ends_with(".json"))
        .map(|name| name[..name.len() - 5].to_string())
        .collect())
}
