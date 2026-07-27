//! Runtime-history machinery for the makespan matcher, split out of
//! `makespan/mod.rs` so that module stays focused (Python split it to keep
//! `makespan/__init__.py` under the 300-line file-size limit — the guard
//! fired when the capacity-aware assignment guard was added 2026-05-17).
//! Mean per-(model,task) runtime is rebuilt from completed/ blobs on a TTL
//! and used to order the queue (LPT) and project agent finish times. This
//! module has NO dependency on makespan's matcher functions so the
//! dependency is one-directional and cannot cycle.
//!
//! Port of `stado/scheduler/makespan/_history.py`.

use std::collections::HashMap;
use std::sync::LazyLock;
use std::time::{Duration, Instant};

use chrono::DateTime;
use regex::Regex;
use serde_json::Value;
use tokio::sync::Mutex;

use crate::queue::{JobStorage, StorageError};

/// Python `HISTORY_TTL_S`.
pub const HISTORY_TTL_S: u64 = 600;
/// Don't scan every completed/ blob each refresh. Python
/// `COMPLETED_SAMPLE_CAP` (note: 4000 here, distinct from the 6000 in
/// `constants::COMPLETED_SAMPLE_CAP` used by the sizing maps).
pub const COMPLETED_SAMPLE_CAP: usize = 4000;

static MODEL_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"--model\s+(\S+)").expect("static regex compiles"));
static TASK_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"--task\s+(\S+)").expect("static regex compiles"));

/// (model, task) -> mean runtime seconds. Python `_history_cache` shape.
pub type History = HashMap<(String, String), f64>;

/// Python `_extract_model_task`: `--model`/`--task` out of a command
/// line, quote-stripped; "" for whichever flag is absent.
pub fn extract_model_task(command: &str) -> (String, String) {
    let model = MODEL_RE
        .captures(command)
        .map(|c| c[1].trim_matches(['\'', '"']).to_string())
        .unwrap_or_default();
    let task = TASK_RE
        .captures(command)
        .map(|c| c[1].trim_matches(['\'', '"']).to_string())
        .unwrap_or_default();
    (model, task)
}

/// Mean runtime in seconds per (model, task), from completed/ blobs.
/// Python `_build_history`.
///
/// Reads at most [`COMPLETED_SAMPLE_CAP`] blobs in parallel. Sequential
/// per-blob downloads at ~50-100ms each are too slow for the Cloud
/// Function's 540s timeout when the cap is in the thousands; parallelism
/// brings the wall time down to seconds.
pub async fn build_history(
    store: &JobStorage,
    log_fn: &dyn Fn(&str),
) -> Result<History, StorageError> {
    let paths: Vec<String> = store
        .list_paths("completed/", 0)
        .await?
        .into_iter()
        .take(COMPLETED_SAMPLE_CAP)
        .collect();
    if paths.is_empty() {
        return Ok(History::new());
    }
    // TOCTOU race: the listing returns a name, then move_job (completed
    // -> failed when verify_command rc != 0, or manual cleanup) deletes
    // the blob before we get here. A missing blob (None) is skipped; any
    // other error propagates so the tick fails visibly on a real problem.
    let texts = super::download_many(store, &paths).await?;

    let mut by_key: HashMap<(String, String), Vec<f64>> = HashMap::new();
    for text in texts.into_iter().flatten() {
        let doc: Value = serde_json::from_str(&text)?;
        let (Some(st), Some(ct)) = (
            doc.get("started_at").and_then(Value::as_str),
            doc.get("completed_at").and_then(Value::as_str),
        ) else {
            continue;
        };
        let started = DateTime::parse_from_rfc3339(st)
            .map_err(|e| StorageError::Other(format!("makespan history: bad started_at {st:?}: {e}")))?;
        let completed = DateTime::parse_from_rfc3339(ct)
            .map_err(|e| StorageError::Other(format!("makespan history: bad completed_at {ct:?}: {e}")))?;
        let elapsed = (completed - started).num_milliseconds() as f64 / 1000.0;
        if elapsed <= 0.0 {
            continue;
        }
        let (model, task) = extract_model_task(doc.get("command").and_then(Value::as_str).unwrap_or(""));
        if model.is_empty() || task.is_empty() {
            continue;
        }
        by_key.entry((model, task)).or_default().push(elapsed);
    }
    let out: History = by_key
        .into_iter()
        .map(|(k, v)| (k, v.iter().sum::<f64>() / v.len() as f64))
        .collect();
    log_fn(&format!(
        "makespan: history rebuilt from {} completed/ blobs, {} (model,task) keys",
        paths.len(),
        out.len()
    ));
    Ok(out)
}

/// TTL cache for the history map. Python `_history_cache` +
/// `_history_cache_built_at` module globals; here a struct so tests can
/// hold isolated instances, with [`global()`] reproducing the
/// module-global for production callers.
pub struct HistoryCache {
    inner: Mutex<(History, Option<Instant>)>,
}

impl Default for HistoryCache {
    fn default() -> Self {
        Self::new()
    }
}

impl HistoryCache {
    pub fn new() -> Self {
        Self { inner: Mutex::new((History::new(), None)) }
    }

    /// Python `_history`: rebuild when the cache is older than
    /// [`HISTORY_TTL_S`] — or empty, so a completed/-less fleet retries
    /// every call instead of pinning an empty map for 10 minutes.
    pub async fn history(
        &self,
        store: &JobStorage,
        log_fn: &dyn Fn(&str),
    ) -> Result<History, StorageError> {
        let mut guard = self.inner.lock().await;
        let (map, built_at) = &*guard;
        let stale =
            built_at.is_none_or(|t| t.elapsed() > Duration::from_secs(HISTORY_TTL_S)) || map.is_empty();
        if stale {
            let rebuilt = build_history(store, log_fn).await?;
            *guard = (rebuilt, Some(Instant::now()));
        }
        Ok(guard.0.clone())
    }
}

/// The process-wide history cache (Python module-global `_history_cache`).
pub fn global() -> &'static HistoryCache {
    static GLOBAL: LazyLock<HistoryCache> = LazyLock::new(HistoryCache::new);
    &GLOBAL
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::queue::local_file::LocalBackend;
    use std::sync::Arc;

    fn store() -> (tempfile::TempDir, JobStorage) {
        let dir = tempfile::tempdir().unwrap();
        let backend = LocalBackend::new(dir.path().to_str().unwrap()).unwrap();
        (dir, JobStorage::with_backend(Arc::new(backend), "local"))
    }

    fn completed(job_id: &str, command: &str, started: &str, completed: &str) -> String {
        serde_json::json!({
            "job_id": job_id,
            "command": command,
            "state": "completed",
            "started_at": started,
            "completed_at": completed,
        })
        .to_string()
    }

    #[test]
    fn extract_model_task_handles_quotes_and_absence() {
        assert_eq!(
            extract_model_task("run --model org/m --task lm-eval"),
            ("org/m".to_string(), "lm-eval".to_string())
        );
        // Python's _TASK_RE is `--task\s+(\S+)` — quoted values stop at the
        // space and keep no special quote handling beyond strip("'\"").
        assert_eq!(
            extract_model_task("run --model 'org/q' --task \"t q\""),
            ("org/q".to_string(), "t".to_string())
        );
        assert_eq!(extract_model_task("nothing"), (String::new(), String::new()));
    }

    #[tokio::test]
    async fn build_history_means_per_model_task() {
        let (_dir, store) = store();
        store
            .upload_text(
                "completed/a.json",
                &completed("a", "x --model m --task t", "2026-01-01T00:00:00+00:00", "2026-01-01T00:01:00+00:00"),
            )
            .await
            .unwrap();
        store
            .upload_text(
                "completed/b.json",
                &completed("b", "x --model m --task t", "2026-01-01T00:00:00+00:00", "2026-01-01T00:03:00+00:00"),
            )
            .await
            .unwrap();
        store
            .upload_text(
                "completed/c.json",
                &completed("c", "x --model m --task other", "2026-01-01T00:00:00+00:00", "2026-01-01T00:00:30+00:00"),
            )
            .await
            .unwrap();
        // No (model, task) -> skipped; non-positive elapsed -> skipped;
        // missing timestamps -> skipped.
        store
            .upload_text(
                "completed/d.json",
                &completed("d", "admin --restart", "2026-01-01T00:00:00+00:00", "2026-01-01T00:01:00+00:00"),
            )
            .await
            .unwrap();
        store
            .upload_text(
                "completed/e.json",
                &completed("e", "x --model m --task t", "2026-01-01T00:01:00+00:00", "2026-01-01T00:00:00+00:00"),
            )
            .await
            .unwrap();
        store
            .upload_text("completed/f.json", &serde_json::json!({"job_id": "f", "command": "x --model m --task t"}).to_string())
            .await
            .unwrap();

        let h = build_history(&store, &|_| ()).await.unwrap();
        assert_eq!(h.len(), 2);
        assert_eq!(h[&("m".to_string(), "t".to_string())], 120.0); // (60+180)/2
        assert_eq!(h[&("m".to_string(), "other".to_string())], 30.0);
    }

    #[tokio::test]
    async fn empty_completed_gives_empty_history() {
        let (_dir, store) = store();
        assert!(build_history(&store, &|_| ()).await.unwrap().is_empty());
        // The TTL cache does not pin an empty map: it rebuilds next call.
        let cache = HistoryCache::new();
        assert!(cache.history(&store, &|_| ()).await.unwrap().is_empty());
        store
            .upload_text(
                "completed/a.json",
                &completed("a", "x --model m --task t", "2026-01-01T00:00:00+00:00", "2026-01-01T00:01:00+00:00"),
            )
            .await
            .unwrap();
        let h = cache.history(&store, &|_| ()).await.unwrap();
        assert_eq!(h.len(), 1);
        // ...and a warm cache does NOT rebuild (blob deleted, map survives).
        store.delete_blob("completed/a.json").await.unwrap();
        let h2 = cache.history(&store, &|_| ()).await.unwrap();
        assert_eq!(h2.len(), 1);
    }
}
