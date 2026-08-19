//! The 'run' primitive: the tracking entity above a job.
//!
//! Port of `stado/queue/runs/__init__.py`. One `wc submit` invocation = one
//! run. Written once to `runs/<run_id>.json` with the member job_ids and
//! submitter provenance, then never mutated (static manifest — avoids GCS
//! read-modify-write contention across the fleet). Run status is *derived*
//! from the members' current prefixes, so "is run X done?" costs
//! O(run size) targeted reads instead of an O(whole-queue) scan.

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

/// Inputs for the immutable run manifest. Python
/// `write_run_manifest(store, run_id, name, submitter_app, submitted_by,
/// submitted_from, commands, job_ids)`.
pub struct RunManifest<'a> {
    pub run_id: &'a str,
    /// Explicit name (`WC_RUN_NAME`); `None`/empty auto-derives from commands.
    pub name: Option<&'a str>,
    /// Orchestrator name; `None`/empty becomes "manual".
    pub submitter_app: Option<&'a str>,
    pub submitted_by: &'a str,
    pub submitted_from: &'a str,
    pub commands: &'a [String],
    pub job_ids: &'a [String],
}

/// Write the immutable run manifest. Called once after all member jobs are
/// queued so job_ids is complete.
pub async fn write_run_manifest(
    store: &JobStorage,
    manifest: &RunManifest<'_>,
) -> Result<(), StorageError> {
    let name = match manifest.name {
        Some(name) if !name.is_empty() => name.to_string(),
        _ => derive_run_name(manifest.commands),
    };
    let submitter_app = match manifest.submitter_app {
        Some(app) if !app.is_empty() => app,
        _ => "manual",
    };
    // Key order matches the Python dict; json.dumps(indent=2) maps to
    // serde_json::to_string_pretty.
    let mut body = Map::new();
    body.insert("run_id".into(), Value::from(manifest.run_id));
    body.insert("name".into(), Value::from(name));
    body.insert(
        "created_at".into(),
        Value::from(Utc::now().format("%Y-%m-%dT%H:%M:%S+00:00").to_string()),
    );
    body.insert("submitter_app".into(), Value::from(submitter_app));
    body.insert("submitted_by".into(), Value::from(manifest.submitted_by));
    body.insert(
        "submitted_from".into(),
        Value::from(manifest.submitted_from),
    );
    body.insert("n_jobs".into(), Value::from(manifest.job_ids.len()));
    body.insert(
        "job_ids".into(),
        Value::Array(
            manifest
                .job_ids
                .iter()
                .map(|j| Value::from(j.as_str()))
                .collect(),
        ),
    );
    body.insert(
        "commands".into(),
        Value::Array(
            manifest
                .commands
                .iter()
                .map(|c| Value::from(c.as_str()))
                .collect(),
        ),
    );
    store
        .upload_text(
            &format!("{RUN_PREFIX}/{}.json", manifest.run_id),
            &serde_json::to_string_pretty(&Value::Object(body))?,
        )
        .await
}

/// Read a run manifest; `None` when it does not exist.
pub async fn read_run(
    store: &JobStorage,
    run_id: &str,
) -> Result<Option<Map<String, Value>>, StorageError> {
    let Some(raw) = store
        .download_text(&format!("{RUN_PREFIX}/{run_id}.json"))
        .await?
    else {
        return Ok(None);
    };
    let value: Value = serde_json::from_str(&raw)?;
    match value {
        Value::Object(map) => Ok(Some(map)),
        _ => Err(StorageError::Other(format!(
            "run manifest {run_id} is not a JSON object"
        ))),
    }
}

/// Which prefix currently holds this job_id, or None if absent.
async fn job_state(store: &JobStorage, job_id: &str) -> Result<Option<&'static str>, StorageError> {
    for prefix in ALL_PREFIXES {
        if store.read_job(prefix, job_id).await?.is_some() {
            return Ok(Some(prefix));
        }
    }
    Ok(None)
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
    let job_ids: Vec<String> = manifest
        .get("job_ids")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|j| j.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    let mut counts: BTreeMap<String, i64> =
        ALL_PREFIXES.iter().map(|p| (p.to_string(), 0)).collect();
    let mut missing = 0;
    for job_id in &job_ids {
        match job_state(store, job_id).await? {
            Some(prefix) => *counts.get_mut(prefix).expect("prefix initialized") += 1,
            None => missing += 1,
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
        n_jobs: manifest
            .get("n_jobs")
            .and_then(Value::as_i64)
            .unwrap_or(job_ids.len() as i64),
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

