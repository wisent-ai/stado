//! Runtime-history machinery for the makespan matcher, split out of
//! `makespan/mod.rs` so that module stays focused and under the 300-line
//! file-size limit.
//! Mean per-(model,task) runtime is rebuilt from completed/ blobs whenever
//! the completed/ listing changed, and used to order the queue (LPT) and
//! project agent finish times. This
//! module has NO dependency on makespan's matcher functions so the
//! dependency is one-directional and cannot cycle.
//!
//! Port of `stado/scheduler/makespan/_history.py`.

use std::collections::HashMap;
use std::sync::LazyLock;

use chrono::DateTime;
use regex::Regex;
use serde_json::Value;
use tokio::sync::Mutex;

use crate::queue::{JobStorage, StorageError};

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

/// Mean runtime in seconds per (model, task), from the completed/ blobs
/// named in `paths`. Python `_build_history`.
///
/// Reads every completed blob, in parallel; no sample count is chosen here.
async fn build_history(
    store: &JobStorage,
    paths: &[String],
    log_fn: &dyn Fn(&str),
) -> Result<History, StorageError> {
    if paths.is_empty() {
        return Ok(History::new());
    }
    // TOCTOU race: the listing returns a name, then move_job (completed
    // -> failed when verify_command rc != 0, or manual cleanup) deletes
    // the blob before we get here. A missing blob (None) is skipped; any
    // other error propagates so the tick fails visibly on a real problem.
    let texts = super::download_many(store, paths).await?;

    let mut by_key: HashMap<(String, String), Vec<f64>> = HashMap::new();
    for text in texts.into_iter().flatten() {
        let doc: Value = serde_json::from_str(&text)?;
        let (Some(st), Some(ct)) = (
            doc.get("started_at").and_then(Value::as_str),
            doc.get("completed_at").and_then(Value::as_str),
        ) else {
            continue;
        };
        let started = DateTime::parse_from_rfc3339(st).map_err(|e| {
            StorageError::Other(format!("makespan history: bad started_at {st:?}: {e}"))
        })?;
        let completed = DateTime::parse_from_rfc3339(ct).map_err(|e| {
            StorageError::Other(format!("makespan history: bad completed_at {ct:?}: {e}"))
        })?;
        let elapsed = (completed - started).num_milliseconds() as f64 / 1000.0;
        if elapsed <= 0.0 {
            continue;
        }
        let (model, task) =
            extract_model_task(doc.get("command").and_then(Value::as_str).unwrap_or(""));
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

/// The history map and the completed/ listing it was built from. Python
/// `_history_cache`; here a struct so tests can hold isolated instances,
/// with [`global()`] reproducing the module-global for production callers.
pub struct HistoryCache {
    inner: Mutex<(History, Option<Vec<String>>)>,
}

impl Default for HistoryCache {
    fn default() -> Self {
        Self::new()
    }
}

impl HistoryCache {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new((History::new(), None)),
        }
    }

    /// Python `_history`: the listing of completed/ is read on every call
    /// (names only), and the blobs are downloaded again only when it differs
    /// from the listing the held map was built from. No time decides it.
    pub async fn history(
        &self,
        store: &JobStorage,
        log_fn: &dyn Fn(&str),
    ) -> Result<History, StorageError> {
        let paths: Vec<String> = store.list_paths("completed/", 0).await?;
        let mut guard = self.inner.lock().await;
        if guard.1.as_ref() != Some(&paths) {
            let rebuilt = build_history(store, &paths, log_fn).await?;
            *guard = (rebuilt, Some(paths));
        }
        Ok(guard.0.clone())
    }
}

/// The process-wide history cache (Python module-global `_history_cache`).
pub fn global() -> &'static HistoryCache {
    static GLOBAL: LazyLock<HistoryCache> = LazyLock::new(HistoryCache::new);
    &GLOBAL
}
