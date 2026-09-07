//! The memory pass's durable state: where it lives, who may hold it, and how
//! one pass hands the next its interval.
//!
//! The shape is the disk janitor's, key for key — `version`,
//! `last_attempt_at`, `last_attempt_by_writer` and the whole previous
//! `report` — because [`crate::deploy::host_disk`] already reads that shape
//! off a host over the control channel and an operator already knows how to
//! read it. A second layout would be a second thing to learn for no
//! difference in meaning.
//!
//! The lock is this pass's own file, not the disk janitor's. The two share a
//! directory and nothing else: a memory pass queued behind a `build_caches`
//! walk of a large `$HOME` would arrive exactly as late as that walk, and the
//! incident this pass exists for is measured in the minutes before a process
//! cannot allocate.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

use super::constants;
use crate::providers::local::disk_cleanup::{ensure_state_dir, secure_home, JanitorError};

/// Per-writer attempt stamps: `{writer: epoch_seconds}`.
pub const WRITER_ATTEMPTS: &str = "last_attempt_by_writer";

/// The memory state file relative to `$HOME`.
///
/// Exported for the same reason the disk janitor exports its own:
/// [`crate::deploy::host_disk`] reports the memory state of a host it is not
/// running on and has to name the exact file this module maintains.
pub fn state_relative_path() -> String {
    let mut parts: Vec<&str> = constants::STATE_DIR_PARTS.to_vec();
    parts.push(constants::STATE_NAME);
    parts.join("/")
}

/// The memory pass's exclusive run lock relative to `$HOME`.
pub fn lock_relative_path() -> String {
    let mut parts: Vec<&str> = constants::STATE_DIR_PARTS.to_vec();
    parts.push(constants::LOCK_NAME);
    parts.join("/")
}

/// The state directory for this host, created and permission-checked by the
/// disk janitor's own guard so both passes agree on what a safe state
/// directory is.
pub fn state_dir() -> Result<PathBuf, JanitorError> {
    let home = secure_home(&crate::config_file::expand_tilde("~"))?;
    ensure_state_dir(&home)
}

/// An exclusive flock held for the duration of one pass.
pub struct PassLock {
    file: Option<File>,
}

impl Drop for PassLock {
    fn drop(&mut self) {
        if let Some(file) = self.file.take() {
            let _ = fs2::FileExt::unlock(&file);
        }
    }
}

/// Take the pass lock, or report that another pass holds it.
///
/// `Ok(None)` is the `lock_busy` outcome and is not an error: two writers run
/// this pass by design — the janitor unit on its own timer and the queue
/// agent's janitor task — and the one that arrives second must record that it
/// did nothing rather than run a second repair against the same host.
pub fn acquire_pass_lock(state_dir: &Path) -> Result<Option<PassLock>, JanitorError> {
    let path = state_dir.join(constants::LOCK_NAME);
    let file = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(&path)
        .map_err(|error| JanitorError::os(&error.to_string()))?;
    match fs2::FileExt::try_lock_exclusive(&file) {
        Ok(()) => Ok(Some(PassLock { file: Some(file) })),
        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => Ok(None),
        Err(error) => Err(JanitorError::os(&error.to_string())),
    }
}

/// Read the persisted state document, or an empty object when none exists.
pub fn read_state(state_dir: &Path) -> Value {
    let path = state_dir.join(constants::STATE_NAME);
    let Ok(text) = std::fs::read_to_string(&path) else {
        return Value::Object(Map::new());
    };
    serde_json::from_str(&text).unwrap_or_else(|_| Value::Object(Map::new()))
}

/// Read the persisted state for an explicit home, for readers that are not
/// the pass itself.
pub fn read_state_in(home: &Path) -> Value {
    let mut path = home.to_path_buf();
    for part in constants::STATE_DIR_PARTS {
        path = path.join(part);
    }
    let path = path.join(constants::STATE_NAME);
    let Ok(text) = std::fs::read_to_string(&path) else {
        return Value::Object(Map::new());
    };
    serde_json::from_str(&text).unwrap_or_else(|_| Value::Object(Map::new()))
}

/// The last time this writer attempted a pass, in epoch seconds.
///
/// Per writer, not shared, and that distinction is load-bearing: the disk
/// janitor learned it on 2026-08-31, when two janitors on one host gated each
/// other out of every pass because they read one another's stamp.
pub fn writer_last_attempt(state: &Value, writer: &str) -> Option<f64> {
    state
        .get(WRITER_ATTEMPTS)
        .and_then(Value::as_object)
        .and_then(|writers| writers.get(writer))
        .and_then(Value::as_f64)
}

/// Persist this pass's report, its attempt stamp and the writer that made it.
///
/// Written through a temporary file and renamed, so a reader never sees half
/// a document, and `last_attempt_at` never moves backwards: an
/// `interval_noop` anchored on an older stamp must not rewind a newer pass by
/// another writer.
pub fn write_state(
    state_dir: &Path,
    report: &Value,
    writer: &str,
    attempted_at: f64,
) -> Result<(), JanitorError> {
    let previous = read_state(state_dir);
    let mut state = previous.as_object().cloned().unwrap_or_else(Map::new);
    let mut writers = previous
        .get(WRITER_ATTEMPTS)
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_else(Map::new);
    writers.insert(writer.to_string(), serde_json::json!(attempted_at));
    let last_attempt_at = previous
        .get("last_attempt_at")
        .and_then(Value::as_f64)
        .map_or(attempted_at, |recorded| recorded.max(attempted_at));
    state.insert("version".to_string(), Value::from(constants::STATE_VERSION));
    state.insert(
        "last_attempt_at".to_string(),
        serde_json::json!(last_attempt_at),
    );
    state.insert(WRITER_ATTEMPTS.to_string(), Value::Object(writers));
    state.insert("report".to_string(), report.clone());
    let document = Value::Object(state);
    let path = state_dir.join(constants::STATE_NAME);
    let staged = state_dir.join(format!("{}.staged", constants::STATE_NAME));
    let text = format!("{}\n", serde_json::to_string_pretty(&document)?);
    {
        let mut file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&staged)
            .map_err(|error| JanitorError::os(&error.to_string()))?;
        file.write_all(text.as_bytes())
            .map_err(|error| JanitorError::os(&error.to_string()))?;
        file.sync_all()
            .map_err(|error| JanitorError::os(&error.to_string()))?;
    }
    std::fs::rename(&staged, &path).map_err(|error| JanitorError::os(&error.to_string()))?;
    Ok(())
}

/// Epoch seconds as the state file records them.
pub fn now_epoch_seconds() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs_f64())
        .unwrap_or_default()
}
