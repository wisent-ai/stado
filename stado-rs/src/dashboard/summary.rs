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
        other => Err(StorageError::Other(format!("invalid free_slots value {other}"))),
    }
}

/// Return parsed capacity/<consumer>.json blobs, most recent first
/// (Python `_read_capacity_blobs`; see the module doc for the backend
/// deviation).
async fn read_capacity_blobs(store: &JobStorage) -> Result<Vec<Value>, StorageError> {
    let mut blobs: Vec<Value> = Vec::new();
    for info in store.list_blobs_with_meta("capacity/").await? {
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
            info.updated.map(isoformat_utc).map_or(Value::Null, Value::String),
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
            let row = by_model_state.entry(model.clone()).or_insert_with(|| {
                json!({"queue": 0, "running": 0, "completed": 0, "failed": 0})
            });
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
    let mut live_agents: Vec<Value> = Vec::new();
    let mut stale_agents: Vec<Value> = Vec::new();
    for c in &capacity {
        let published = parse_iso(c.get("published_at").and_then(Value::as_str))?;
        let age = published.map(|p| (now - p).num_microseconds().unwrap_or(0) as f64 / 1e6);
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

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::queue::LocalBackend;

    pub(crate) fn store() -> (tempfile::TempDir, JobStorage) {
        let dir = tempfile::tempdir().expect("tempdir");
        let backend = LocalBackend::new(dir.path().to_str().expect("utf8 path")).expect("backend");
        let store =
            JobStorage::with_backend_and_bucket(Arc::new(backend), "local", "test-bucket");
        (dir, store)
    }

    fn job(job_id: &str, command: &str) -> Job {
        Job::new(job_id, command)
    }

    /// The summarizer over fabricated LocalBackend blobs: counts, per-model
    /// breakdown, recent failed/completed, live vs stale agents, throughput.
    #[tokio::test]
    async fn summarize_over_fabricated_blobs() {
        let (_dir, store) = store();

        let q1 = job("queue0001", "python run.py --model llama-8b --task extract");
        let q2 = job("queue0002", "python run.py --model llama-8b --task extract");
        let q3 = job("queue0003", "python run.py --model qwen-7b --task steer");
        store.write_job("queue", &q1).await.unwrap();
        store.write_job("queue", &q2).await.unwrap();
        store.write_job("queue", &q3).await.unwrap();

        store
            .write_job("running", &job("running001", "python run.py --model llama-8b --task extract"))
            .await
            .unwrap();

        let mut done = job("done00001", "python run.py --model qwen-7b --task steer");
        done.started_at = Some("2026-07-01T00:00:00+00:00".into());
        done.completed_at = Some("2026-07-01T00:10:00+00:00".into());
        store.write_job("completed", &done).await.unwrap();

        let mut failed = job("failed0001", "python run.py --model llama-8b --task extract");
        failed.error = Some(format!("{}TAIL", "x".repeat(300)));
        store.write_job("failed", &failed).await.unwrap();

        // One fresh capacity blob, one stale (published long ago).
        store
            .upload_text(
                "capacity/agent-fresh.json",
                &json!({
                    "consumer_id": "local-fresh",
                    "kind": "local",
                    "free_slots": {"nvidia-l4": 2},
                    "free_vram_gb": 20,
                    "total_vram_gb": 24,
                    "published_at": isoformat_utc(Utc::now()),
                })
                .to_string(),
            )
            .await
            .unwrap();
        store
            .upload_text(
                "capacity/agent-stale.json",
                &json!({
                    "consumer_id": "local-stale",
                    "kind": "local",
                    "free_slots": {"nvidia-l4": 1},
                    "published_at": "2020-01-01T00:00:00+00:00",
                })
                .to_string(),
            )
            .await
            .unwrap();

        let summary = summarize(&store).await.unwrap();
        assert_eq!(summary["bucket"], "test-bucket");
        assert_eq!(summary["counts"]["queue"], 3);
        assert_eq!(summary["counts"]["running"], 1);
        assert_eq!(summary["counts"]["completed"], 1);
        assert_eq!(summary["counts"]["failed"], 1);

        let models = summary["by_model_state"].as_object().unwrap();
        assert_eq!(
            models["llama-8b"],
            json!({"queue": 2, "running": 1, "completed": 0, "failed": 1})
        );
        assert_eq!(models["qwen-7b"], json!({"queue": 1, "running": 0, "completed": 1, "failed": 0}));

        assert_eq!(summary["recent_failed"][0]["job_id"], "failed0001");
        assert_eq!(summary["recent_failed"][0]["error"].as_str().unwrap().len(), 240);
        assert_eq!(summary["completed_recent"][0]["job_id"], "done00001");
        assert_eq!(summary["completed_recent"][0]["wall_seconds"], 600.0);

        let live = summary["live_agents"].as_array().unwrap();
        assert_eq!(live.len(), 1);
        assert_eq!(live[0]["consumer_id"], "local-fresh");
        assert_eq!(summary["stale_agents"].as_array().unwrap().len(), 1);
        assert_eq!(summary["stale_agents"][0]["consumer_id"], "local-stale");

        // avg wall 600s * queue depth 3 / 2 live slots = 900s projected.
        assert_eq!(summary["throughput"]["avg_wall_seconds_per_completed_job"], 600.0);
        assert_eq!(summary["throughput"]["samples"], 1);
        assert_eq!(summary["throughput"]["live_total_free_slots"], 2);
        assert_eq!(summary["throughput"]["projected_remaining_seconds"], 900.0);

        // Fast prefix counts agree (and ignore non-JSON blobs).
        store.upload_text("queue/notes.txt", "not a job").await.unwrap();
        let counts = fast_counts(&store).await.unwrap();
        assert_eq!(counts["queue"], 3);
        assert_eq!(counts["running"], 1);
    }

    #[tokio::test]
    async fn summarize_empty_store_has_null_projection() {
        let (_dir, store) = store();
        let summary = summarize(&store).await.unwrap();
        assert_eq!(summary["counts"]["queue"], 0);
        assert!(summary["throughput"]["avg_wall_seconds_per_completed_job"].is_null());
        assert!(summary["throughput"]["projected_remaining_seconds"].is_null());
        assert_eq!(summary["live_agents"], json!([]));
    }

    #[test]
    fn model_and_task_parsing_matches_python_regexes() {
        assert_eq!(model_of("run --model 'llama-8b' --task x"), "llama-8b");
        assert_eq!(model_of("run --model \"qwen-7b\" --task x"), "qwen-7b");
        assert_eq!(model_of("run --model plain --task x"), "plain");
        assert_eq!(model_of("no model here"), "(unknown)");
        assert_eq!(task_of("run --task extract_activations"), "extract_activations");
        assert_eq!(task_of("no task"), "(unknown)");
    }
}
