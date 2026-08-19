//! Generic job-completion coverage verifier + retry orchestrator.
//!
//! Port of `stado/coverage/__init__.py`, `stado/coverage/failures.py` (the
//! [`failures`] submodule), and `stado/coverage/cli.py` ([`cli_main`]).
//!
//! Universe-agnostic: nothing here knows about activation extraction,
//! training, eval, or any specific job type. A Universe yields
//! [`UniverseEntry`] tuples (group_key, command, expected_uri) and supplies
//! a [`Verifier`]. The orchestrator walks the universe, checks each
//! expected output, diffs against state, re-submits the gap subset via
//! `queue::submit`, and tracks per-group_key attempts at
//! `<COVERAGE_STATE_PREFIX>/<universe_id>/state.json`. After
//! `COVERAGE_ATTEMPT_CAP` attempts a group_key is UNFIXABLE and surfaced
//! but not re-submitted.
//!
//! DEVIATION: Python discovers Universe classes via importlib.metadata
//! entry_points group `stado.coverage_universes`. Rust has no entry-point
//! analog, so discovery is a static in-process registry: downstream crates
//! call [`register_universe`] at startup with a factory. The `stado` crate
//! itself ships no universes (they are external plugins in Python too), so
//! the registry is empty by default and every universe id is "unknown".
//! The unknown-universe error message is byte-identical to Python's.

use std::collections::BTreeMap;
use std::sync::{LazyLock, Mutex};

use async_trait::async_trait;
use futures::StreamExt;
use serde_json::{json, Map, Value};

use crate::config;
use crate::models::{json_dumps_pretty_sorted, py_str_repr};
use crate::queue::submit::{submit_job, SubmitOptions};
use crate::queue::{JobStorage, StorageError};

/// Python `PRESENT` — verifier outcome: the expected output exists.
pub const PRESENT: &str = "present";
/// Python `MISSING` — verifier outcome: the expected output is absent.
pub const MISSING: &str = "missing";
/// Python `UNFIXABLE` — group_key exhausted COVERAGE_ATTEMPT_CAP attempts.
pub const UNFIXABLE: &str = "unfixable";

/// Coverage-layer error. Python raises `ValueError` (verifier misuse),
/// `RuntimeError` (HTTP retry-cap), and lets urllib/storage/submit
/// exceptions propagate; the variants here cover those.
#[derive(Debug, thiserror::Error)]
pub enum CoverageError {
    #[error("{0}")]
    Other(String),
    #[error(transparent)]
    Storage(#[from] StorageError),
    #[error(transparent)]
    Submit(#[from] crate::queue::submit::SubmitError),
    #[error(transparent)]
    Http(#[from] reqwest::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

impl From<&str> for CoverageError {
    fn from(msg: &str) -> Self {
        Self::Other(msg.to_string())
    }
}

impl From<String> for CoverageError {
    fn from(msg: String) -> Self {
        Self::Other(msg)
    }
}

/// One expected job in a coverage-bound batch (Python `UniverseEntry`).
/// `group_key` uniquely identifies the entry within the universe (the
/// state-file key); `command` is the shell command that would produce
/// `expected_uri` when it succeeds; `extra` carries per-entry arguments
/// forwarded to submit on retry (only `verify_command` is consumed here).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UniverseEntry {
    pub group_key: String,
    pub command: String,
    pub expected_uri: String,
    pub extra: Map<String, Value>,
}

impl UniverseEntry {
    pub fn new(
        group_key: impl Into<String>,
        command: impl Into<String>,
        expected_uri: impl Into<String>,
    ) -> Self {
        Self {
            group_key: group_key.into(),
            command: command.into(),
            expected_uri: expected_uri.into(),
            extra: Map::new(),
        }
    }
}

/// Returns whether an expected output URI is present (Python `Verifier`).
/// Implementations return [`PRESENT`] or [`MISSING`] and raise on transport
/// errors so the orchestrator fails fast rather than silently retrying.
#[async_trait]
pub trait Verifier: Send + Sync {
    async fn check(&self, expected_uri: &str) -> Result<String, CoverageError>;
}

/// HEAD against an http(s) URI; optional bearer token for HF/private
/// (Python `URIExistsVerifier`). status < 400 -> PRESENT, 404 -> MISSING,
/// 429 -> backoff `COVERAGE_VERIFY_BACKOFF_BASE ** attempt` and retry up to
/// `COVERAGE_HTTP_RETRY_CAP` times; anything else raises.
pub struct URIExistsVerifier {
    bearer_token: String,
    client: reqwest::Client,
}

impl URIExistsVerifier {
    pub fn new(bearer_token: impl Into<String>) -> Self {
        Self {
            bearer_token: bearer_token.into(),
            client: reqwest::Client::new(),
        }
    }
}

#[async_trait]
impl Verifier for URIExistsVerifier {
    async fn check(&self, expected_uri: &str) -> Result<String, CoverageError> {
        for attempt in 0..config::COVERAGE_HTTP_RETRY_CAP {
            let mut request = self.client.head(expected_uri);
            if !self.bearer_token.is_empty() {
                request = request.header("Authorization", format!("Bearer {}", self.bearer_token));
            }
            let response = request.send().await?;
            let status = response.status().as_u16();
            if status < 400 {
                return Ok(PRESENT.to_string());
            }
            if status == 404 {
                return Ok(MISSING.to_string());
            }
            if status == 429 {
                let backoff =
                    config::COVERAGE_VERIFY_BACKOFF_BASE.pow(u32::try_from(attempt).unwrap_or(31));
                tokio::time::sleep(std::time::Duration::from_secs(backoff as u64)).await;
                continue;
            }
            // Python re-raises the urllib HTTPError for other statuses.
            return Err(CoverageError::Other(format!(
                "HEAD {expected_uri}: HTTP {status}"
            )));
        }
        Err(format!("HEAD {expected_uri}: retry-cap exceeded").into())
    }
}

/// Existence check for a provider-neutral `stado://<namespace>/<key>` object
/// through the backend selected by `STADO_CONFIG`.
pub struct StadoObjectExistsVerifier {
    store: JobStorage,
}

impl StadoObjectExistsVerifier {
    pub fn new(store: JobStorage) -> Self {
        Self { store }
    }

    /// Resolve the configured Stado store without accepting a provider locator.
    pub async fn with_default_store() -> Result<Self, CoverageError> {
        Ok(Self::new(JobStorage::new().await?))
    }
}

#[async_trait]
impl Verifier for StadoObjectExistsVerifier {
    async fn check(&self, expected_uri: &str) -> Result<String, CoverageError> {
        let object = crate::object_store::ObjectRef::parse(expected_uri)
            .map_err(|error| CoverageError::Other(error.to_string()))?;
        let txt = self.store.download_text(&object.storage_path()).await?;
        Ok(if txt.is_some() {
            PRESENT.to_string()
        } else {
            MISSING.to_string()
        })
    }
}

/// Submitter-defined contract describing the expected batch (Python
/// `Universe` ABC).
pub trait Universe: Send + Sync {
    /// Stable identifier for state-file scoping. URL-safe.
    fn id(&self) -> &str;
    /// Every (group_key, command, expected_uri) tuple.
    fn iter_entries(&self) -> Vec<UniverseEntry>;
    /// Verifier used to check expected_uri for entries from this universe.
    fn verifier(&self) -> Box<dyn Verifier>;
    /// Forwarded to submit on retry (Python `submit_kwargs()`); override
    /// for provider/priority/etc. Default = submit defaults.
    fn submit_options(&self) -> SubmitOptions {
        SubmitOptions::default()
    }
}

/// Constructor for a Universe from CLI `--kv` kwargs. The `Err` string is
/// surfaced as the CLI error message (Python `TypeError` from a bad
/// constructor kwarg surfaces as a traceback; here it is a clean error).
pub type UniverseFactory =
    Box<dyn Fn(Map<String, Value>) -> Result<Box<dyn Universe>, String> + Send + Sync>;

static UNIVERSES: LazyLock<Mutex<BTreeMap<String, UniverseFactory>>> =
    LazyLock::new(|| Mutex::new(BTreeMap::new()));

/// Register a universe factory under `name` (the static-registry analog of
/// a Python `stado.coverage_universes` entry point). Later registrations
/// with the same name replace earlier ones.
pub fn register_universe(name: impl Into<String>, factory: UniverseFactory) {
    UNIVERSES
        .lock()
        .expect("universe registry poisoned")
        .insert(name.into(), factory);
}

/// Registered universe ids, sorted (Python `list_universes` /
/// `sorted(discover_universes())`).
pub fn registered_universe_names() -> Vec<String> {
    UNIVERSES
        .lock()
        .expect("universe registry poisoned")
        .keys()
        .cloned()
        .collect()
}

/// Python `list_universes`.
pub fn list_universes() -> Vec<String> {
    registered_universe_names()
}

/// The exact click UsageError message Python raises for an unknown
/// universe id: `unknown universe {id!r}. Registered: {sorted or '(none)'}`.
pub fn unknown_universe_message(universe_id: &str) -> String {
    let names = registered_universe_names();
    let registered = if names.is_empty() {
        "(none)".to_string()
    } else {
        let items: Vec<String> = names.iter().map(|n| py_str_repr(n)).collect();
        format!("[{}]", items.join(", "))
    };
    format!(
        "unknown universe {}. Registered: {registered}",
        py_str_repr(universe_id)
    )
}

/// Instantiate a registered universe from CLI kwargs (Python
/// `_build_universe`).
pub fn build_universe(
    universe_id: &str,
    kwargs: Map<String, Value>,
) -> Result<Box<dyn Universe>, String> {
    {
        let registry = UNIVERSES.lock().expect("universe registry poisoned");
        if let Some(factory) = registry.get(universe_id) {
            // Called under the lock: factories must not re-enter the
            // registry (they construct, they don't register).
            return factory(kwargs);
        }
    } // Drop the guard before unknown_universe_message re-locks.
    Err(unknown_universe_message(universe_id))
}

// ---------------------------------------------------------------------------
// state (<COVERAGE_STATE_PREFIX>/<universe_id>/state.json)
// ---------------------------------------------------------------------------

fn state_path(universe_id: &str) -> String {
    format!("{}/{universe_id}/state.json", config::COVERAGE_STATE_PREFIX)
}

/// Python `state_load`: `{}` when the state blob is absent; corrupt JSON
/// propagates as an error like Python `json.loads`.
pub async fn state_load(store: &JobStorage, universe_id: &str) -> Result<Value, CoverageError> {
    let Some(txt) = store.download_text(&state_path(universe_id)).await? else {
        return Ok(Value::Object(Map::new()));
    };
    Ok(serde_json::from_str(&txt)?)
}

/// Python `state_save`: `json.dumps(state, indent=2, sort_keys=True)`.
pub async fn state_save(
    store: &JobStorage,
    universe_id: &str,
    state: &Value,
) -> Result<(), CoverageError> {
    store
        .upload_text(&state_path(universe_id), &json_dumps_pretty_sorted(state))
        .await?;
    Ok(())
}

/// Mutable access to `state[group_key]`, creating the (object) slot when
/// missing — Python `state.setdefault(group_key, {})`.
fn state_slot<'a>(state: &'a mut Value, group_key: &str) -> &'a mut Map<String, Value> {
    if !state.is_object() {
        *state = Value::Object(Map::new());
    }
    let obj = state.as_object_mut().expect("ensured object");
    let slot = obj
        .entry(group_key.to_string())
        .or_insert_with(|| Value::Object(Map::new()));
    if !slot.is_object() {
        *slot = Value::Object(Map::new());
    }
    slot.as_object_mut().expect("ensured object")
}

// ---------------------------------------------------------------------------
// verify / retry orchestrator
// ---------------------------------------------------------------------------

/// Python `CoverageReport` dataclass.
#[derive(Debug, Clone)]
pub struct CoverageReport {
    pub universe_id: String,
    pub total_entries: usize,
    pub present: usize,
    pub missing: usize,
    pub unfixable: Vec<(String, String)>,
    pub gaps: Vec<UniverseEntry>,
    pub opaque: Vec<UniverseEntry>,
}

impl CoverageReport {
    /// Python `CoverageReport.as_dict()` (key order is the Python dict
    /// literal order — the CLI prints it unsorted).
    pub fn as_dict(&self) -> Value {
        json!({
            "universe_id": self.universe_id,
            "total_entries": self.total_entries,
            "present": self.present,
            "missing": self.missing,
            "unfixable_count": self.unfixable.len(),
            "gap_count": self.gaps.len(),
            "opaque_count": self.opaque.len(),
        })
    }
}

/// Python `verify`: walk the universe in parallel (thread pool ->
/// `buffer_unordered(threads)`), classify each entry against `state`,
/// build a report. Progress is logged every
/// `COVERAGE_PROGRESS_LOG_EVERY` completed entries.
pub async fn verify(
    universe: &dyn Universe,
    threads: usize,
    state: &Value,
    log: Option<&dyn Fn(String)>,
) -> Result<CoverageReport, CoverageError> {
    let entries = universe.iter_entries();
    let total = entries.len();
    let verifier = universe.verifier();
    let results: Vec<Result<(UniverseEntry, String), CoverageError>> =
        futures::stream::iter(entries)
            .map(|entry| {
                let verifier = &verifier;
                async move {
                    let status = verifier.check(&entry.expected_uri).await?;
                    Ok((entry, status))
                }
            })
            .buffer_unordered(threads)
            .collect()
            .await;

    let mut present_n = 0usize;
    let mut gaps: Vec<UniverseEntry> = Vec::new();
    let mut unfixable: Vec<(String, String)> = Vec::new();
    for (index, result) in results.into_iter().enumerate() {
        let (entry, status) = result?;
        let done = index + 1;
        if let Some(log) = log {
            if done as i64 % config::COVERAGE_PROGRESS_LOG_EVERY == 0 {
                log(format!("[{}] {done}/{total}", universe.id()));
            }
        }
        if status == PRESENT {
            present_n += 1;
            continue;
        }
        let slot = state.get(&entry.group_key);
        let attempts = slot
            .and_then(|s| s.get("attempts"))
            .and_then(Value::as_i64)
            .unwrap_or(0);
        if attempts >= config::COVERAGE_ATTEMPT_CAP {
            let last_err = slot
                .and_then(|s| s.get("last_error"))
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            unfixable.push((entry.group_key.clone(), last_err));
            continue;
        }
        gaps.push(entry);
    }
    Ok(CoverageReport {
        universe_id: universe.id().to_string(),
        total_entries: total,
        present: present_n,
        missing: total - present_n,
        unfixable,
        gaps,
        opaque: Vec::new(),
    })
}

/// Python `retry_gaps`: submit each gap entry via `submit_job` so the
/// per-entry `verify_command` (from `entry.extra`) reaches the agent —
/// `submit_batch` flattens kwargs across all jobs and cannot carry it.
/// Without it the agent marks the job COMPLETED on exit=0 even if no
/// expected output was produced. Returns the number of submitted jobs.
pub async fn retry_gaps(
    universe: &dyn Universe,
    report: &CoverageReport,
    state: &mut Value,
    store: &JobStorage,
    batch_label: &str,
    log: Option<&dyn Fn(String)>,
) -> Result<usize, CoverageError> {
    if report.gaps.is_empty() {
        return Ok(0);
    }
    let batch_id = if batch_label.is_empty() {
        format!(
            "coverage-retry-{}-{}",
            universe.id(),
            chrono::Utc::now().timestamp()
        )
    } else {
        batch_label.to_string()
    };
    let base = universe.submit_options();
    let mut submitted = 0usize;
    for gap in &report.gaps {
        let verify_command = gap
            .extra
            .get("verify_command")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let options = SubmitOptions {
            batch_id: batch_id.clone(),
            bucket: config::bucket().to_string(),
            verify_command,
            ..base.clone()
        };
        submit_job(&gap.command, &options).await?;
        submitted += 1;
    }
    let now = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();
    for entry in &report.gaps {
        let slot = state_slot(state, &entry.group_key);
        let attempts = slot.get("attempts").and_then(Value::as_i64).unwrap_or(0);
        slot.insert("attempts".into(), Value::from(attempts + 1));
        slot.insert("last_batch_id".into(), Value::from(batch_id.as_str()));
        slot.insert("last_submitted_at".into(), Value::from(now.as_str()));
    }
    state_save(store, universe.id(), state).await?;
    if let Some(log) = log {
        log(format!(
            "[{}] submitted {submitted}/{} in batch {batch_id}",
            universe.id(),
            report.gaps.len()
        ));
    }
    Ok(submitted)
}

/// Python `verify_and_retry`: load state -> verify -> if execute, retry
/// gaps. The store comes from `config::bucket()` like Python
/// `JobStorage(BUCKET)`.
pub async fn verify_and_retry(
    universe: &dyn Universe,
    execute: bool,
    threads: usize,
    log: Option<&dyn Fn(String)>,
) -> Result<CoverageReport, CoverageError> {
    let store = JobStorage::with_bucket(config::bucket()).await?;
    verify_and_retry_with_store(universe, &store, execute, threads, log).await
}

/// [`verify_and_retry`] with an explicit store (offline/test seam; the
/// Python equivalent of constructing `JobStorage` yourself).
pub async fn verify_and_retry_with_store(
    universe: &dyn Universe,
    store: &JobStorage,
    execute: bool,
    threads: usize,
    log: Option<&dyn Fn(String)>,
) -> Result<CoverageReport, CoverageError> {
    let mut state = state_load(store, universe.id()).await?;
    let report = verify(universe, threads, &state, log).await?;
    if execute {
        retry_gaps(universe, &report, &mut state, store, "", log).await?;
    }
    Ok(report)
}

// ---------------------------------------------------------------------------
// failures.py — bridge from failed/ blob store -> coverage state
// ---------------------------------------------------------------------------

/// Port of `stado/coverage/failures.py`. The Job model does not (yet)
/// carry a `coverage_universe_id` / `coverage_group_key` field, so
/// failures cannot be propagated to the universe state file from inside
/// the coordinator's running -> failed transition. Until that field
/// lands, [`failures::scan_failed_commands`] provides the back-reference.
pub mod failures {
    use super::*;

    /// Python `FAILED_PREFIX`.
    pub const FAILED_PREFIX: &str = "failed/";
    /// Python `ERROR_PREVIEW_MAX`.
    pub const ERROR_PREVIEW_MAX: usize = 1024;

    /// Python `s[:n]` on a `str` (character-based).
    fn truncate_chars(s: &str, n: usize) -> String {
        s.chars().take(n).collect()
    }

    /// Python `_load_failed_blob`: corrupt/absent blobs become None.
    async fn load_failed_blob(
        store: &JobStorage,
        path: &str,
    ) -> Result<Option<Value>, CoverageError> {
        let Some(txt) = store.download_text(path).await? else {
            return Ok(None);
        };
        Ok(serde_json::from_str(&txt).ok())
    }

    /// Python `scan_failed_commands`: `{command: most_recent_failure_record}`
    /// from `failed/`. With `command_prefix`, only failed blobs whose
    /// `.command` starts with the prefix are kept. The
    /// most-recent-by-failed_at record wins on duplicate commands.
    pub async fn scan_failed_commands(
        store: &JobStorage,
        command_prefix: Option<&str>,
        threads: usize,
    ) -> Result<BTreeMap<String, Map<String, Value>>, CoverageError> {
        let infos = store.list_blobs_with_meta(FAILED_PREFIX).await?;
        let paths: Vec<String> = infos
            .into_iter()
            .map(|info| info.name)
            .filter(|name| name.ends_with(".json"))
            .collect();
        let blobs: Vec<Result<Option<Value>, CoverageError>> = futures::stream::iter(paths)
            .map(|path| async move { load_failed_blob(store, &path).await })
            .buffer_unordered(threads)
            .collect()
            .await;
        let mut out: BTreeMap<String, Map<String, Value>> = BTreeMap::new();
        for blob in blobs {
            let Some(blob) = blob? else { continue };
            let Some(cmd) = blob
                .get("command")
                .and_then(Value::as_str)
                .filter(|cmd| !cmd.is_empty())
            else {
                continue;
            };
            if let Some(prefix) = command_prefix {
                if !cmd.starts_with(prefix) {
                    continue;
                }
            }
            let ts = blob
                .get("failed_at")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            let wins = match out.get(cmd) {
                None => true,
                Some(prev) => {
                    prev.get("failed_at").and_then(Value::as_str).unwrap_or("") < ts.as_str()
                }
            };
            if !wins {
                continue;
            }
            let error = blob.get("error").and_then(Value::as_str).unwrap_or("");
            let record = Map::from_iter([
                (
                    "error".to_string(),
                    Value::from(truncate_chars(error, ERROR_PREVIEW_MAX)),
                ),
                ("failed_at".to_string(), Value::from(ts)),
                (
                    "job_id".to_string(),
                    Value::from(blob.get("job_id").and_then(Value::as_str).unwrap_or("")),
                ),
                (
                    "batch_id".to_string(),
                    Value::from(blob.get("batch_id").and_then(Value::as_str).unwrap_or("")),
                ),
            ]);
            out.insert(cmd.to_string(), record);
        }
        Ok(out)
    }

    /// Python `correlate_failures_into_state`: pre-seed the universe's
    /// coverage state with last_error/last_failure_at pulled from the
    /// failed/ index, so the next `verify` promotes the group_key to
    /// UNFIXABLE with the real error string. Returns the merged state
    /// (also persisted to storage when anything matched). Unlike the
    /// Python signature the store is required (the `None` default only
    /// constructed `JobStorage(BUCKET)`).
    pub async fn correlate_failures_into_state(
        universe: &dyn Universe,
        store: &JobStorage,
        state: Option<Value>,
        command_prefix: Option<&str>,
    ) -> Result<Value, CoverageError> {
        let mut state = match state {
            Some(state) => state,
            None => state_load(store, universe.id()).await?,
        };
        let failed = scan_failed_commands(
            store,
            command_prefix,
            config::COVERAGE_VERIFY_THREADS as usize,
        )
        .await?;
        if failed.is_empty() {
            return Ok(state);
        }
        let mut matched = 0usize;
        for entry in universe.iter_entries() {
            let Some(rec) = failed.get(&entry.command) else {
                continue;
            };
            let slot = state_slot(&mut state, &entry.group_key);
            slot.insert("last_error".into(), rec["error"].clone());
            slot.insert("last_failure_at".into(), rec["failed_at"].clone());
            slot.insert("last_failed_job_id".into(), rec["job_id"].clone());
            slot.insert("last_failed_batch_id".into(), rec["batch_id"].clone());
            matched += 1;
        }
        if matched > 0 {
            state_save(store, universe.id(), &state).await?;
        }
        Ok(state)
    }

    /// Python `record_failure`: forward write of a job's terminal error
    /// against its universe state. Idempotent under repeated calls for the
    /// same group_key (overwrites last_error).
    pub async fn record_failure(
        universe_id: &str,
        group_key: &str,
        error_text: &str,
        store: &JobStorage,
    ) -> Result<(), CoverageError> {
        let mut state = state_load(store, universe_id).await?;
        let slot = state_slot(&mut state, group_key);
        slot.insert(
            "last_error".into(),
            Value::from(truncate_chars(error_text, ERROR_PREVIEW_MAX)),
        );
        slot.insert(
            "last_failure_at".into(),
            Value::from(chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string()),
        );
        state_save(store, universe_id, &state).await
    }

    /// Python `matched_failed_jids_for_universe`: `{group_key:
    /// failed_job_id}` for entries whose command has a matching failed/
    /// blob.
    pub async fn matched_failed_jids_for_universe(
        universe: &dyn Universe,
        store: &JobStorage,
        command_prefix: Option<&str>,
    ) -> Result<BTreeMap<String, String>, CoverageError> {
        let failed = scan_failed_commands(
            store,
            command_prefix,
            config::COVERAGE_VERIFY_THREADS as usize,
        )
        .await?;
        let mut out = BTreeMap::new();
        for entry in universe.iter_entries() {
            if let Some(rec) = failed.get(&entry.command) {
                out.insert(
                    entry.group_key.clone(),
                    rec.get("job_id")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string(),
                );
            }
        }
        Ok(out)
    }

    /// Python `iter_failed_commands`: the (command, failure_record) pairs
    /// of [`scan_failed_commands`], as a streaming-iterator analog.
    pub async fn iter_failed_commands(
        store: &JobStorage,
        command_prefix: Option<&str>,
    ) -> Result<Vec<(String, Map<String, Value>)>, CoverageError> {
        let scanned = scan_failed_commands(
            store,
            command_prefix,
            config::COVERAGE_VERIFY_THREADS as usize,
        )
        .await?;
        Ok(scanned.into_iter().collect())
    }
}

// ---------------------------------------------------------------------------
// coverage/cli.py — `stado-coverage` click group
// ---------------------------------------------------------------------------

/// Python `_coerce`: KEY=VALUE -> typed value. Comma-list (stripped,
/// empties dropped), int, or str.
pub fn coerce(value: &str) -> Value {
    if value.contains(',') {
        return Value::Array(
            value
                .split(',')
                .map(str::trim)
                .filter(|part| !part.is_empty())
                .map(|part| Value::from(part.to_string()))
                .collect(),
        );
    }
    match python_int(value) {
        Some(int) => Value::from(int),
        None => Value::from(value),
    }
}

/// Python `int(str)` semantics used by `_coerce`: optional sign, `_`
/// digit separators between digits.
fn python_int(raw: &str) -> Option<i64> {
    let digits = raw.strip_prefix(['+', '-']).unwrap_or(raw);
    if digits.is_empty()
        || digits.starts_with('_')
        || digits.ends_with('_')
        || digits.contains("__")
        || !digits.chars().all(|c| c.is_ascii_digit() || c == '_')
    {
        return None;
    }
    let cleaned: String = raw.chars().filter(|&c| c != '_').collect();
    cleaned.parse::<i64>().ok()
}

/// Python `_kv_to_kwargs`: repeated `--kv KEY=VALUE` flags -> constructor
/// kwargs. The `Err` string is the click UsageError message.
pub fn kv_to_kwargs(pairs: &[String]) -> Result<Map<String, Value>, String> {
    let mut out = Map::new();
    for pair in pairs {
        let Some((key, value)) = pair.split_once('=') else {
            return Err(format!("--kv expects KEY=VALUE, got {}", py_str_repr(pair)));
        };
        out.insert(key.trim().to_string(), coerce(value.trim()));
    }
    Ok(out)
}

/// Print `value` as Python `json.dumps(value, indent=2)` (insertion
/// order, ensure_ascii) on stdout.
fn print_pretty(value: &Value) {
    let pretty = serde_json::to_string_pretty(value).expect("JSON serialization is infallible");
    println!("{}", crate::models::ensure_ascii(&pretty));
}

/// click UsageError rendering for a subcommand: usage + Try line + blank +
/// `Error: {msg}` on stderr, exit 2.
fn usage_error(command: &str, message: &str) -> i32 {
    eprintln!("Usage: stado-coverage {command} [OPTIONS] UNIVERSE_ID");
    eprintln!("Try 'stado-coverage {command} --help' for help.");
    eprintln!();
    eprintln!("Error: {message}");
    2
}

/// A runtime failure after argument parsing (Python: uncaught exception
/// traceback, exit 1; here a clean `Error: {msg}` line, same exit code).
fn runtime_error(err: &CoverageError) -> i32 {
    eprintln!("Error: {err}");
    1
}

#[derive(clap::Parser)]
#[command(
    name = "stado-coverage",
    about = "Verify + retry job-completion coverage for a registered universe."
)]
struct Cli {
    #[command(subcommand)]
    command: CoverageCommands,
}

#[derive(clap::Subcommand)]
enum CoverageCommands {
    /// List registered coverage universes.
    List,
    /// Dry-run coverage walk; print per-universe report JSON. No submits.
    Verify {
        universe_id: String,
        /// Universe constructor kwarg KEY=VALUE; repeat per kwarg.
        #[arg(long = "kv")]
        kv_pairs: Vec<String>,
    },
    /// Verify, and with --execute, re-submit MISSING tuples via submit_batch.
    Retry {
        universe_id: String,
        /// Universe constructor kwarg KEY=VALUE; repeat per kwarg.
        #[arg(long = "kv")]
        kv_pairs: Vec<String>,
        /// Actually submit gap jobs via submit_batch; default is dry-run.
        #[arg(long)]
        execute: bool,
    },
}

/// The `stado-coverage` entry point (click group). Exit codes match click:
/// 2 for usage errors (clap parse failures and UsageError equivalents),
/// 1 for runtime failures and the empty-universe `list`, 0 on success.
pub async fn cli_main() -> i32 {
    let cli = <Cli as clap::Parser>::parse();
    let log = |msg: String| eprintln!("{msg}");
    match cli.command {
        CoverageCommands::List => {
            let names = list_universes();
            if names.is_empty() {
                eprintln!("(no universes registered)");
                return 1;
            }
            for name in names {
                println!("{name}");
            }
            0
        }
        CoverageCommands::Verify {
            universe_id,
            kv_pairs,
        } => {
            let universe = match kv_to_kwargs(&kv_pairs)
                .and_then(|kwargs| build_universe(&universe_id, kwargs))
            {
                Ok(universe) => universe,
                Err(message) => return usage_error("verify", &message),
            };
            let result = async {
                let store = JobStorage::with_bucket(config::bucket()).await?;
                let state = state_load(&store, universe.id()).await?;
                verify(
                    universe.as_ref(),
                    config::COVERAGE_VERIFY_THREADS as usize,
                    &state,
                    Some(&log),
                )
                .await
            }
            .await;
            match result {
                Ok(report) => {
                    print_pretty(&report.as_dict());
                    0
                }
                Err(err) => runtime_error(&err),
            }
        }
        CoverageCommands::Retry {
            universe_id,
            kv_pairs,
            execute,
        } => {
            let universe = match kv_to_kwargs(&kv_pairs)
                .and_then(|kwargs| build_universe(&universe_id, kwargs))
            {
                Ok(universe) => universe,
                Err(message) => return usage_error("retry", &message),
            };
            match verify_and_retry(
                universe.as_ref(),
                execute,
                config::COVERAGE_VERIFY_THREADS as usize,
                Some(&log),
            )
            .await
            {
                Ok(report) => {
                    print_pretty(&report.as_dict());
                    0
                }
                Err(err) => runtime_error(&err),
            }
        }
    }
}

