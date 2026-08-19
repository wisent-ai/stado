//! Dashboard summary/parsing helpers — port of
//! `stado/dashboard_summary/__init__.py` (`_fast_counts` + `_summarize`).
//!
//! The Python module was split out of dashboard.py so the cheap counts path
//! can answer without an extra hop; splitting also unblocked removing the
//! broad `except Exception: pass` blocks that previously absorbed
//! corrupt-blob JSON decodes and slot-count int parses. Each removed
//! silent-except now raises so the dashboard surfaces ingest errors instead
//! of quietly under-reporting fleet state — corrupt blobs and unparseable
//! timestamps therefore propagate as errors here too (and kill the refresh
//! loop, which keeps serving the last good snapshot).
//!
//! DEVIATION (deliberate): Python `_read_capacity_blobs` returns [] when
//! `store._sdk_bucket is None` (any non-GCS backend) — an artifact of the
//! SDK-vs-backend split. The crate routes every backend through the unified
//! [`BlobBackend`], so capacity blobs are read for the local backend too
//! and the local control plane's dashboard can see its in-process agent.

use std::collections::BTreeMap;
use std::sync::LazyLock;

use chrono::{DateTime, Utc};
use regex::Regex;
use serde_json::{json, Map, Value};

use crate::config;
use crate::models::{isoformat_utc, Job};
use crate::queue::{JobStorage, StorageError};

static MODEL_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"--model\s+['\"]?([^'\"\s]+)"#).expect("model regex"));
static TASK_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"--task\s+(\S+)").expect("task regex"));

/// Python `_parse_iso`: `datetime.fromisoformat(ts.replace("Z", "+00:00"))`;
/// `None`/empty -> `None`. Garbage raises in Python (killing the refresh
/// loop); here it surfaces as an error for the same effect.
fn parse_iso(ts: Option<&str>) -> Result<Option<DateTime<Utc>>, StorageError> {
    let Some(ts) = ts else { return Ok(None) };
    if ts.is_empty() {
        return Ok(None);
    }
    let parsed = DateTime::parse_from_rfc3339(&ts.replace('Z', "+00:00"))
        .map_err(|exc| StorageError::Other(format!("invalid ISO timestamp {ts:?}: {exc}")))?;
    Ok(Some(parsed.with_timezone(&Utc)))
}

/// Python `_wall_seconds`: completed/failed minus started, clamped at 0.
fn wall_seconds(job: &Job) -> Result<Option<f64>, StorageError> {
    let start = parse_iso(job.started_at.as_deref())?;
    let end = parse_iso(job.completed_at.as_deref())?.or(parse_iso(job.failed_at.as_deref())?);
    let (Some(start), Some(end)) = (start, end) else {
        return Ok(None);
    };
    let seconds = (end - start).num_microseconds().unwrap_or(0) as f64 / 1e6;
    Ok(Some(seconds.max(0.0)))
}

/// Python `_model_of`.
fn model_of(cmd: &str) -> String {
    MODEL_RE
        .captures(cmd)
        .and_then(|caps| caps.get(1).map(|m| m.as_str().to_string()))
        .unwrap_or_else(|| "(unknown)".to_string())
}

/// Python `_task_of`.
fn task_of(cmd: &str) -> String {
    TASK_RE
        .captures(cmd)
        .and_then(|caps| caps.get(1).map(|m| m.as_str().to_string()))
        .unwrap_or_else(|| "(unknown)".to_string())
}

/// Python `int(n)` for capacity free_slots values: numbers truncate toward
/// zero, strings parse; anything else raises.
fn py_int(value: &Value) -> Result<i64, StorageError> {
    match value {
        Value::Number(n) => Ok(n.as_f64().unwrap_or(0.0) as i64),
        Value::String(s) => s
            .trim()
            .parse::<i64>()
            .map_err(|exc| StorageError::Other(format!("invalid free_slots value {s:?}: {exc}"))),
        other => Err(StorageError::Other(format!(
            "invalid free_slots value {other}"
        ))),
    }
}

/// Where capacity reports live for this deployment.
///
/// Writers publish them through the object API, which stores product objects
/// under `ecosystem/<namespace>/`. This reader kept listing the bare `capacity/`
/// prefix from before namespacing, so it found only blobs abandoned by the
/// migration -- days old on every host -- and the operator's screen reported a
/// dead fleet ("No capacity report exists for this registered worker") while
/// every worker was publishing on schedule. `beacon_object_path` already
/// resolves the same way for host health; this is the same rule for capacity,
/// so writer and reader cannot drift apart again.
fn capacity_prefix() -> String {
    let namespace = crate::config::wc_stado_storage_namespace();
    if namespace.trim().is_empty() {
        return "capacity/".to_string();
    }
    format!("{}{namespace}/capacity/", crate::object_store::ROOT_PREFIX)
}

/// Return parsed capacity/<consumer>.json blobs, most recent first
/// (Python `_read_capacity_blobs`; see the module doc for the backend
/// deviation).
async fn read_capacity_blobs(store: &JobStorage) -> Result<Vec<Value>, StorageError> {
    let mut blobs = Vec::new();
    for info in store.list_blobs_with_meta(&capacity_prefix()).await? {
        if !info.name.ends_with(".json") {
            continue;
        }
        let Some(text) = store.download_text(&info.name).await? else {
            continue;
        };
        // Strict-raise parity: corrupt capacity JSON kills the refresh loop
        // instead of quietly under-reporting fleet state.
        let mut data: Value = serde_json::from_str(&text)?;
        let Some(map) = data.as_object_mut() else {
            return Err(StorageError::Other(format!(
                "capacity blob {} is not a JSON object",
                info.name
            )));
        };
        map.insert("_blob_name".to_string(), json!(info.name));
        map.insert(
            "_blob_updated".to_string(),
            info.updated
                .map(isoformat_utc)
                .map_or(Value::Null, Value::String),
        );
        blobs.push(data);
    }
    blobs.sort_by(|a, b| {
        let key_a = a.get("published_at").and_then(Value::as_str).unwrap_or("");
        let key_b = b.get("published_at").and_then(Value::as_str).unwrap_or("");
        key_b.cmp(key_a)
    });
    Ok(blobs)
}

fn capacity_age(capacity: &Value, now: DateTime<Utc>) -> Result<Option<f64>, StorageError> {
    let published = parse_iso(capacity.get("published_at").and_then(Value::as_str))?;
    Ok(published.map(|time| (now - time).num_microseconds().unwrap_or(0) as f64 / 1e6))
}

fn capacity_projection(
    capacity: Option<&Value>,
    age_seconds: Option<f64>,
    fresh_cutoff_seconds: f64,
) -> Map<String, Value> {
    let (status, reason) = match capacity {
        Some(_) if age_seconds.is_some_and(|age| age <= fresh_cutoff_seconds) => (
            "live",
            "Capacity report is within the freshness window.".to_string(),
        ),
        Some(_) => (
            "stale",
            format!(
                "Capacity report is older than the {}-second freshness window.",
                fresh_cutoff_seconds as i64
            ),
        ),
        None => (
            "unavailable",
            "No capacity report exists for this registered worker.".to_string(),
        ),
    };
    let mut projection = Map::new();
    projection.insert("status".into(), json!(status));
    projection.insert("availability_reason".into(), json!(reason));
    projection.insert(
        "consumer_id".into(),
        capacity
            .and_then(|value| value.get("consumer_id"))
            .cloned()
            .unwrap_or(Value::Null),
    );
    projection.insert(
        "free_slots".into(),
        capacity
            .and_then(|value| value.get("free_slots"))
            .cloned()
            .unwrap_or_else(|| json!({})),
    );
    projection.insert(
        "free_vram_gb".into(),
        capacity
            .and_then(|value| value.get("free_vram_gb"))
            .cloned()
            .unwrap_or(Value::Null),
    );
    projection.insert(
        "total_vram_gb".into(),
        capacity
            .and_then(|value| value.get("total_vram_gb"))
            .cloned()
            .unwrap_or(Value::Null),
    );
    projection.insert(
        "published_at".into(),
        capacity
            .and_then(|value| value.get("published_at"))
            .cloned()
            .unwrap_or(Value::Null),
    );
    projection.insert("age_seconds".into(), json!(age_seconds));
    projection
}

async fn registered_workers(
    store: &JobStorage,
    capacity: &[Value],
    now: DateTime<Utc>,
    fresh_cutoff_seconds: f64,
) -> Result<Vec<Value>, StorageError> {
    let registry = match store.download_text(crate::targets::REGISTRY_BLOB).await? {
        Some(text) => Some(
            crate::targets::load_registry_from_str(&text)
                .map_err(|error| StorageError::Other(format!("invalid registry.json: {error}")))?,
        ),
        None => None,
    };
    let mut capacity_by_target = BTreeMap::<String, usize>::new();
    if let Some(registry) = &registry {
        for (index, report) in capacity.iter().enumerate() {
            if report.get("kind").and_then(Value::as_str) != Some("local") {
                continue;
            }
            let Some(consumer_id) = report.get("consumer_id").and_then(Value::as_str) else {
                continue;
            };
            let host = consumer_id
                .split_once('-')
                .map_or(consumer_id, |(_, host)| host);
            let target = registry
                .lookup_self(host)
                .map_err(|error| StorageError::Other(error.to_string()))?;
            if let Some(target) = target {
                capacity_by_target
                    .entry(target.name.clone())
                    .or_insert(index);
            }
        }
    }

    let mut matched_capacity = vec![false; capacity.len()];
    let mut workers = Vec::new();
    if let Some(registry) = &registry {
        for target in registry
            .targets
            .iter()
            .filter(|target| target.kind == "local")
        {
            let capacity_index = capacity_by_target.get(&target.name).copied();
            let report = capacity_index.map(|index| {
                matched_capacity[index] = true;
                &capacity[index]
            });
            let age_seconds = report
                .map(|value| capacity_age(value, now))
                .transpose()?
                .flatten();
            let mut worker = Map::new();
            worker.insert("target_name".into(), json!(target.name));
            worker.insert("declared".into(), json!(true));
            worker.insert("kind".into(), json!(target.kind));
            worker.insert("hostnames".into(), json!(target.hostnames));
            worker.insert("gpu_type".into(), json!(target.gpu_type));
            worker.insert("role".into(), json!(target.role));
            worker.extend(capacity_projection(
                report,
                age_seconds,
                fresh_cutoff_seconds,
            ));
            workers.push(Value::Object(worker));
        }
    }

    for (index, report) in capacity.iter().enumerate() {
        if matched_capacity[index] {
            continue;
        }
        let age_seconds = capacity_age(report, now)?;
        let mut worker = Map::new();
        worker.insert("target_name".into(), Value::Null);
        worker.insert("declared".into(), json!(false));
        worker.insert(
            "kind".into(),
            report.get("kind").cloned().unwrap_or(Value::Null),
        );
        worker.insert("hostnames".into(), json!([]));
        worker.insert("gpu_type".into(), Value::Null);
        worker.insert("role".into(), Value::Null);
        worker.extend(capacity_projection(
            Some(report),
            age_seconds,
            fresh_cutoff_seconds,
        ));
        if let Some(reason) = worker.get_mut("availability_reason") {
            let current = reason.as_str().unwrap_or_default();
            *reason = json!(format!(
                "{current} No registered worker matches this capacity identity."
            ));
        }
        workers.push(Value::Object(worker));
    }
    Ok(workers)
}

/// Count blobs per state prefix without downloading job JSONs (Python
/// `_fast_counts`). Used by the cheap-render path so /api/state.json can
/// return SOMETHING while the full per-job summary is still building.
pub async fn fast_counts(store: &JobStorage) -> Result<Map<String, Value>, StorageError> {
    let mut out = Map::new();
    for prefix in ["queue", "running", "completed", "failed"] {
        let paths = store.list_paths(&format!("{prefix}/"), 0).await?;
        let count = paths.iter().filter(|p| p.ends_with(".json")).count() as i64;
        out.insert(prefix.to_string(), json!(count));
    }
    Ok(out)
}

/// The full fleet summary (Python `_summarize`): downloads every job blob.
/// Never runs inline with a request — the refresh loop caches it.
pub async fn summarize(store: &JobStorage) -> Result<Value, StorageError> {
    let all_jobs = store.list_all_jobs().await?;
    let mut counts = Map::new();
    for (state, jobs) in &all_jobs {
        counts.insert(state.clone(), json!(jobs.len() as i64));
    }

    let mut by_model_state = Map::new();
    let mut recent_failed: Vec<Value> = Vec::new();
    let mut completed_walls: Vec<f64> = Vec::new();
    let mut completed_recent: Vec<Value> = Vec::new();
    for (state, jobs) in &all_jobs {
        for job in jobs {
            let model = model_of(&job.command);
            let row = by_model_state
                .entry(model.clone())
                .or_insert_with(|| json!({"queue": 0, "running": 0, "completed": 0, "failed": 0}));
            if let Some(counter) = row.get_mut(state.as_str()) {
                *counter = json!(counter.as_i64().unwrap_or(0) + 1);
            }
            if state == "completed" {
                let wall = wall_seconds(job)?;
                if let Some(wall) = wall {
                    completed_walls.push(wall);
                }
                if completed_recent.len() < 200 {
                    completed_recent.push(json!({
                        "job_id": job.job_id,
                        "model": model,
                        "task": task_of(&job.command),
                        "wall_seconds": wall,
                        "completed_at": job.completed_at,
                    }));
                }
            } else if state == "failed" && recent_failed.len() < 30 {
                let error = job.error.clone().unwrap_or_default();
                recent_failed.push(json!({
                    "job_id": job.job_id,
                    "model": model,
                    "task": task_of(&job.command),
                    "error": error.chars().take(240).collect::<String>(),
                }));
            }
        }
    }
    completed_recent.sort_by(|a, b| {
        let key_a = a.get("completed_at").and_then(Value::as_str).unwrap_or("");
        let key_b = b.get("completed_at").and_then(Value::as_str).unwrap_or("");
        key_b.cmp(key_a)
    });
    completed_recent.truncate(30);

    let capacity = read_capacity_blobs(store).await?;
    let now = Utc::now();
    let fresh_cutoff_seconds = config::dashboard_agent_fresh_seconds() as f64;
    let workers = registered_workers(store, &capacity, now, fresh_cutoff_seconds).await?;
    let mut live_agents: Vec<Value> = Vec::new();
    let mut stale_agents: Vec<Value> = Vec::new();
    for c in &capacity {
        let age = capacity_age(c, now)?;
        let entry = json!({
            "consumer_id": c.get("consumer_id"),
            "kind": c.get("kind"),
            "free_slots": c.get("free_slots").cloned().unwrap_or_else(|| json!({})),
            "free_vram_gb": c.get("free_vram_gb"),
            "total_vram_gb": c.get("total_vram_gb"),
            "published_at": c.get("published_at"),
            "age_seconds": age,
            "diag": c.get("diag").cloned().unwrap_or_else(|| json!({})),
        });
        if age.is_some_and(|age| age <= fresh_cutoff_seconds) {
            live_agents.push(entry);
        } else {
            stale_agents.push(entry);
        }
    }

    // Throughput-based projection: mean wall * queue_depth / live worker
    // parallelism. If we have no live agents, projection is None.
    let avg_wall = if completed_walls.is_empty() {
        None
    } else {
        Some(completed_walls.iter().sum::<f64>() / completed_walls.len() as f64)
    };
    let mut live_slots: i64 = 0;
    for agent in &live_agents {
        if let Some(free_slots) = agent.get("free_slots").and_then(Value::as_object) {
            for n in free_slots.values() {
                live_slots += py_int(n)?;
            }
        }
    }
    let queue_depth = counts.get("queue").and_then(Value::as_i64).unwrap_or(0) as f64;
    let projected_remaining_seconds = match avg_wall {
        Some(avg) if avg != 0.0 && live_slots > 0 => Some(avg * queue_depth / live_slots as f64),
        _ => None,
    };

    Ok(json!({
        "now": isoformat_utc(now),
        "bucket": store.bucket_name(),
        "counts": Value::Object(counts),
        "by_model_state": Value::Object(by_model_state),
        "live_agents": live_agents,
        "stale_agents": stale_agents,
        "workers": workers,
        "recent_failed": recent_failed,
        "completed_recent": completed_recent,
        "throughput": {
            "avg_wall_seconds_per_completed_job": avg_wall,
            "samples": completed_walls.len() as i64,
            "live_total_free_slots": live_slots,
            "projected_remaining_seconds": projected_remaining_seconds,
        },
    }))
}

