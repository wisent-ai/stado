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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::Job;
    use crate::queue::local_file::LocalBackend;
    use std::sync::Arc;

    fn store() -> (tempfile::TempDir, JobStorage) {
        let dir = tempfile::tempdir().unwrap();
        let backend = LocalBackend::new(dir.path().to_str().unwrap()).unwrap();
        (dir, JobStorage::with_backend(Arc::new(backend), "local"))
    }

    fn strings(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn run_id_format() {
        let id = generate_run_id();
        let parts: Vec<&str> = id.split('-').collect();
        assert_eq!(parts[0], "run");
        assert!(parts[1].parse::<i64>().unwrap() > 0);
        assert_eq!(parts[2].len(), 8);
        assert!(parts[2].chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn derive_name_module_model_tasks() {
        let commands = strings(&[
            "python -m wisent.scripts.train --model 'org/model-x' --task t1",
            "python -m wisent.scripts.train --model org/model-x --task t2",
        ]);
        assert_eq!(derive_run_name(&commands), "train:model-x:t1+t2:2jobs");

        // More than 3 distinct tasks collapse to a count.
        let many: Vec<String> = (0..5)
            .map(|i| format!("python -m a.b.mod --model m --task task{i}"))
            .collect();
        assert_eq!(derive_run_name(&many), "mod:m:5tasks:5jobs");

        // No recognizable flags: just the job count.
        assert_eq!(derive_run_name(&strings(&["echo hi", "echo yo"])), "2jobs");
    }

    #[tokio::test]
    async fn manifest_write_read_and_status_derivation() {
        let (_dir, store) = store();
        let commands = strings(&["python -m a.b.train --task x"]);
        let job_ids = strings(&["ja", "jb"]);
        let manifest = RunManifest {
            run_id: "run-1-abcdef01",
            name: None,
            submitter_app: None,
            submitted_by: "alice",
            submitted_from: "host1",
            commands: &commands,
            job_ids: &job_ids,
        };
        write_run_manifest(&store, &manifest).await.unwrap();

        let read = read_run(&store, "run-1-abcdef01").await.unwrap().unwrap();
        assert_eq!(read["name"], Value::from("train:x:1jobs"));
        assert_eq!(read["submitter_app"], Value::from("manual"));
        assert_eq!(read["n_jobs"], Value::from(2));
        assert_eq!(read["job_ids"], serde_json::json!(["ja", "jb"]));

        // One member completed, one queued, one listed-but-missing job.
        let mut ja = Job::new("ja", "echo a");
        ja.created_at = "2026-01-01T00:00:00+00:00".into();
        store.write_job("completed", &ja).await.unwrap();
        let mut jb = Job::new("jb", "echo b");
        jb.created_at = "2026-01-01T00:00:01+00:00".into();
        store.write_job("queue", &jb).await.unwrap();

        let status = run_status(&store, "run-1-abcdef01").await.unwrap().unwrap();
        assert_eq!(status.counts["completed"], 1);
        assert_eq!(status.counts["queue"], 1);
        assert_eq!(status.missing, 0);
        assert_eq!(status.in_flight, 1);
        assert!(!status.all_terminal);
        assert_eq!(status.submitter_app, "manual");

        // Move the queued job to a terminal prefix: run is done.
        store.move_job(&jb, "queue", "failed").await.unwrap();
        let status = run_status(&store, "run-1-abcdef01").await.unwrap().unwrap();
        assert_eq!(status.counts["failed"], 1);
        assert_eq!(status.in_flight, 0);
        assert!(status.all_terminal);

        assert_eq!(list_runs(&store).await.unwrap(), vec!["run-1-abcdef01"]);
        assert_eq!(run_status(&store, "run-nope").await.unwrap(), None);
    }

    #[tokio::test]
    async fn missing_member_jobs_are_counted() {
        let (_dir, store) = store();
        let commands = strings(&["echo"]);
        let job_ids = strings(&["gone"]);
        let manifest = RunManifest {
            run_id: "run-2-00000000",
            name: Some("explicit"),
            submitter_app: Some("app"),
            submitted_by: "bob",
            submitted_from: "host2",
            commands: &commands,
            job_ids: &job_ids,
        };
        write_run_manifest(&store, &manifest).await.unwrap();
        let status = run_status(&store, "run-2-00000000").await.unwrap().unwrap();
        assert_eq!(status.missing, 1);
        assert!(!status.all_terminal);
    }
}
