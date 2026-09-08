//! Live-worker projection for the makespan matcher: which workers are
//! admitting jobs right now, how much VRAM each already owes to running
//! work, and the per-job runtime estimate both paths share. Split out of
//! `makespan/mod.rs`; the matcher itself lives in `matcher`.

use std::collections::{BTreeMap, HashMap};

use chrono::{DateTime, Utc};
use regex::Regex;
use serde_json::Value;
use std::sync::LazyLock;

use crate::queue::{JobStorage, StorageError};

use super::history::{extract_model_task, History};

/// Python `HEARTBEAT_TTL_S`.
pub const HEARTBEAT_TTL_S: i64 = 180;

static INSTANCE_HOST_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[^@]+@(.+)$").expect("static regex compiles"));

/// Per-job runtime estimate in seconds. Returns None when neither an
/// explicit estimate nor a matching history entry is available; the
/// caller leaves the job unassigned with a log naming the missing
/// (model, task). A new (model, task) combo must either set
/// runtime_seconds_estimate at submit time or wait for a sibling job
/// to complete and seed history; the matcher refuses to guess.
/// Python `_estimate_runtime` (factored onto primitives so both the
/// queued-job path and the running-blob seed path share it).
pub(super) fn estimate_runtime(
    command: &str,
    runtime_seconds_estimate: f64,
    history: &History,
) -> Option<f64> {
    if runtime_seconds_estimate > 0.0 {
        return Some(runtime_seconds_estimate);
    }
    let (model, task) = extract_model_task(command);
    if model.is_empty() || task.is_empty() {
        return None;
    }
    history.get(&(model, task)).copied()
}

/// Live-worker projection state.
#[derive(Debug, Default)]
pub struct AgentInfo {
    pub kind: String,
    pub available_accelerators: BTreeMap<String, i64>,
    pub total_vram_gb: i64,
    /// (finish_offset_seconds, vram_gb) per projected active job.
    pub active_jobs: Vec<(f64, i64)>,
}

/// Workers with a fresh publication and an explicit positive admission
/// decision. A live row alone is not capacity: a worker can keep publishing
/// while disk, RAM, CPU, or accelerator state prevents another claim.
pub(super) async fn live_agents(
    store: &JobStorage,
    now: DateTime<Utc>,
) -> Result<BTreeMap<String, AgentInfo>, StorageError> {
    let paths = store.list_paths("capacity/", 0).await?;
    let texts = download_many(store, &paths).await?;
    let mut agents: BTreeMap<String, AgentInfo> = BTreeMap::new();
    for text in texts.into_iter().flatten() {
        let doc: Value = serde_json::from_str(&text)?;
        let cid = doc
            .get("consumer_id")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let pub_at = doc
            .get("published_at")
            .and_then(Value::as_str)
            .unwrap_or("");
        // Python lets a malformed published_at crash the tick (no try).
        let published = DateTime::parse_from_rfc3339(pub_at).map_err(|e| {
            StorageError::Other(format!(
                "makespan: capacity blob has malformed published_at {pub_at:?}: {e}"
            ))
        })?;
        let age = (now - published.with_timezone(&Utc)).num_seconds();
        if age > HEARTBEAT_TTL_S {
            continue;
        }
        if doc.get("accepting_jobs").and_then(Value::as_bool) != Some(true) {
            continue;
        }
        let available_accelerators: BTreeMap<String, i64> = doc
            .get("available_accelerators")
            .and_then(Value::as_object)
            .map(|obj| {
                obj.iter()
                    .map(|(accelerator, count)| {
                        (accelerator.clone(), count.as_i64().unwrap_or_default())
                    })
                    .collect()
            })
            .unwrap_or_default();
        let kind = doc
            .get("kind")
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or_else(|| cid.split('-').next().unwrap_or("").to_string());
        let total_vram_gb = doc
            .get("total_vram_gb")
            .and_then(|v| v.as_i64().or_else(|| v.as_f64().map(|f| f as i64)))
            .unwrap_or(0);
        agents.insert(
            cid,
            AgentInfo {
                kind,
                available_accelerators,
                total_vram_gb,
                active_jobs: vec![],
            },
        );
    }
    Ok(agents)
}

/// For each running/ blob, locate the executing agent (by hostname
/// in instance_ref) and add an active_slot covering its remaining
/// runtime. Without this, a freshly-claimed long job looks invisible
/// and the matcher would pile more work onto an already-loaded agent.
/// Python `_seed_running_jobs`.
pub(super) async fn seed_running_jobs(
    store: &JobStorage,
    agents: &mut BTreeMap<String, AgentInfo>,
    now: DateTime<Utc>,
    history: &History,
    log_fn: &dyn Fn(&str),
) -> Result<(), StorageError> {
    // consumer_id is "<kind>-<hostname>" (queue/capacity.publish_capacity).
    let mut host_to_cid: HashMap<String, String> = HashMap::new();
    for cid in agents.keys() {
        if let Some((_, host)) = cid.split_once('-') {
            host_to_cid.insert(host.to_string(), cid.clone());
        }
    }
    let paths = store.list_paths("running/", 0).await?;
    let texts = download_many(store, &paths).await?;
    for (path, text) in paths.iter().zip(&texts) {
        let Some(text) = text else { continue }; // moved to completed/failed mid-tick
        let doc: Value = serde_json::from_str(text)?;
        let iref = doc
            .get("instance_ref")
            .and_then(Value::as_str)
            .unwrap_or("");
        let Some(caps) = INSTANCE_HOST_RE.captures(iref) else {
            return Err(StorageError::Other(format!(
                "makespan: running blob {path} has malformed instance_ref {iref:?}; \
                 cannot map to consumer_id"
            )));
        };
        let host = &caps[1];
        let Some(cid) = host_to_cid.get(host) else {
            // Running job points to an agent we no longer track as live.
            // Leave its VRAM out of the projection; the reaper will move
            // the running blob to failed on its own pass. Logging only.
            log_fn(&format!(
                "makespan: running {} on dead host {host}; skipping projection",
                doc.get("job_id").and_then(Value::as_str).unwrap_or("")
            ));
            continue;
        };
        let Some(st) = doc.get("started_at").and_then(Value::as_str) else {
            return Err(StorageError::Other(format!(
                "makespan: running blob {path} has no started_at"
            )));
        };
        let started = DateTime::parse_from_rfc3339(st).map_err(|e| {
            StorageError::Other(format!(
                "makespan: running blob {path} bad started_at {st:?}: {e}"
            ))
        })?;
        let elapsed = (now - started.with_timezone(&Utc)).num_milliseconds() as f64 / 1000.0;
        let command = doc.get("command").and_then(Value::as_str).unwrap_or("");
        let runtime_hint = doc
            .get("runtime_seconds_estimate")
            .and_then(|v| v.as_f64().or_else(|| v.as_i64().map(|i| i as f64)))
            .unwrap_or(0.0);
        let Some(est) = estimate_runtime(command, runtime_hint, history) else {
            // Admin/maintenance commands have no parseable (model, task); the
            // matcher can't predict their finish time. Agent-side smi_free
            // enforces actual VRAM at claim time.
            log_fn(&format!(
                "makespan: skip running {} for seeding",
                doc.get("job_id").and_then(Value::as_str).unwrap_or("")
            ));
            continue;
        };
        let remaining = (est - elapsed.max(0.0)).max(0.0);
        let vram = doc
            .get("gpu_mem_gb")
            .and_then(|v| v.as_i64().or_else(|| v.as_f64().map(|f| f as i64)))
            .unwrap_or(0);
        if let Some(info) = agents.get_mut(cid) {
            info.active_jobs.push((remaining, vram));
        }
    }
    Ok(())
}

/// Parallel-download the given blob paths (Python
/// `ThreadPoolExecutor(max_workers=32)` + `pool.map` → `buffered(32)`,
/// path order preserved). A missing blob (TOCTOU between list and
/// download) is None; any other error propagates.
pub(crate) async fn download_many(
    store: &JobStorage,
    paths: &[String],
) -> Result<Vec<Option<String>>, StorageError> {
    use futures::StreamExt;
    futures::stream::iter(paths)
        .map(|path| store.download_text(path))
        .buffered(32)
        .collect::<Vec<Result<Option<String>, StorageError>>>()
        .await
        .into_iter()
        .collect()
}
