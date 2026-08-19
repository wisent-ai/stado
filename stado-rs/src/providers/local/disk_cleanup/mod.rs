//! Registry-authorized, bounded disk cleanup for local compute hosts.
//!
//! Port of `stado/providers/local/disk/cleanup.py` (the "janitor"). Only
//! fixed roots and fixed cleaner implementations exist here. Registry data
//! can select a cleaner and its retention, but can never supply a path or
//! command.
//!
//! Layout: [`safefs`] holds the dir_fd-relative primitives (and the only
//! `unsafe`), [`hf`] the HuggingFace cache eviction, [`weles`] the weles
//! recordings cleanup, [`build_caches`] the eviction of directories a build
//! tool tagged as regenerable, [`chromium_clones`] the eviction of the bundle
//! clones macOS makes to validate Chromium's signature at every launch. This
//! module owns the report model, the sanitized public state, the
//! exclusive/shared lock file, the canonical-policy resolution, and the
//! top-level [`run_cleanup_once`] orchestration.

pub mod build_caches;
pub mod chromium_clones;
pub mod hf;
pub mod safefs;
pub mod weles;

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs::{File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::time::{Instant, SystemTime};

use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

use crate::targets::{self, ComputeTarget, DiskCleanupPolicy};

pub(crate) const GIB: i64 = 1024 * 1024 * 1024;
/// Python `_STATE_VERSION`.
pub const STATE_VERSION: i64 = 1;
/// Python `_STATE_DIR` (`~/.cache/wisent-compute`).
const STATE_DIR_PARTS: [&str; 2] = [".cache", "wisent-compute"];
/// Python `_LOCK_NAME`.
const LOCK_NAME: &str = "disk-cleanup.lock";
/// Python `_STATE_NAME`.
const STATE_NAME: &str = "disk-cleanup-state.json";
/// Python `_DEADLINE_SECONDS`.
const DEADLINE_SECONDS: f64 = 30.0;
/// Python `_MAX_ERRORS`.
const MAX_ERRORS: usize = 16;

/// The janitor's state file relative to `$HOME` — `_STATE_DIR` joined with
/// `_STATE_NAME` in the Python original.
///
/// Exported because [`crate::deploy::host_disk`] reports the cleanup state
/// of a host it is not running on, and has to name the exact file
/// [`ensure_state_dir`] and `write_state` maintain. A second copy of that
/// path living in the deploy layer would be one rename away from silently
/// reporting "never ran" for a host that runs cleanly every minute.
pub fn state_relative_path() -> String {
    let mut parts: Vec<&str> = STATE_DIR_PARTS.to_vec();
    parts.push(STATE_NAME);
    parts.join("/")
}

/// `st_mode & S_IFMT` (Python `stat.S_IFMT`); the mask value is identical
/// on every Unix the port targets.
pub(crate) fn ifmt(mode: u32) -> u32 {
    mode & 0o170000
}
pub(crate) const IFDIR: u32 = 0o040000;
pub(crate) const IFREG: u32 = 0o100000;
pub(crate) const IFLNK: u32 = 0o120000;

// ---------------------------------------------------------------------------
// errors (Python exception type names, bounded — `_error_code`)
// ---------------------------------------------------------------------------

/// A janitor failure carrying the Python exception TYPE NAME the report
/// records (`_error_code(exc) = type(exc).__name__[:80]`) plus a private
/// detail message that never enters the report.
#[derive(Debug)]
pub struct JanitorError {
    pub code: &'static str,
    pub message: String,
}

impl JanitorError {
    pub fn os(message: &str) -> Self {
        Self {
            code: "OSError",
            message: message.to_string(),
        }
    }
    pub fn timeout(message: &str) -> Self {
        Self {
            code: "TimeoutError",
            message: message.to_string(),
        }
    }
    pub fn blocking(message: &str) -> Self {
        Self {
            code: "BlockingIOError",
            message: message.to_string(),
        }
    }
    pub fn lookup(message: &str) -> Self {
        Self {
            code: "LookupError",
            message: message.to_string(),
        }
    }
    pub fn value(message: &str) -> Self {
        Self {
            code: "ValueError",
            message: message.to_string(),
        }
    }
    /// Python `_error_code`: bounded diagnostics without paths, values,
    /// or credentials.
    pub fn error_code(&self) -> String {
        self.code.chars().take(80).collect()
    }
}

impl std::fmt::Display for JanitorError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for JanitorError {}

impl From<io::Error> for JanitorError {
    fn from(exc: io::Error) -> Self {
        let code = match exc.kind() {
            io::ErrorKind::NotFound => "FileNotFoundError",
            io::ErrorKind::PermissionDenied => "PermissionError",
            io::ErrorKind::TimedOut => "TimeoutError",
            io::ErrorKind::WouldBlock => "BlockingIOError",
            _ => "OSError",
        };
        Self {
            code,
            message: exc.to_string(),
        }
    }
}

impl From<serde_json::Error> for JanitorError {
    fn from(exc: serde_json::Error) -> Self {
        Self {
            code: "ValueError",
            message: exc.to_string(),
        }
    }
}

impl From<crate::queue::StorageError> for JanitorError {
    fn from(exc: crate::queue::StorageError) -> Self {
        // The Python fetches via the GCS SDK and lets its exceptions
        // propagate; the report records only the (bounded) type name.
        Self {
            code: "OSError",
            message: exc.to_string(),
        }
    }
}

// ---------------------------------------------------------------------------
// report model (Python's nested report dicts)
// ---------------------------------------------------------------------------

/// Python `_cleaner_report()`.
#[derive(Debug, Clone, Default)]
pub struct CleanerReport {
    pub scanned_items: i64,
    pub eligible_items: i64,
    pub deleted_items: i64,
    pub expected_bytes: i64,
    pub actual_free_delta_bytes: i64,
    pub skipped: BTreeMap<String, i64>,
}

/// Python `report["caps"]`.
#[derive(Debug, Clone, Default)]
pub struct Caps {
    pub bytes: bool,
    pub items: bool,
    pub scan: bool,
    pub deadline: bool,
}

impl Caps {
    pub fn any(&self) -> bool {
        self.bytes || self.items || self.scan || self.deadline
    }
}

/// Python `_base_report(...)`.
#[derive(Debug, Clone)]
pub struct CleanupReport {
    pub hostname: String,
    pub target_name: Option<String>,
    pub policy_digest: Option<String>,
    pub mode: Option<String>,
    pub check_interval_seconds: Option<i64>,
    pub started_at: String,
    pub duration_ms: i64,
    pub outcome: String,
    pub free_bytes_before: Option<i64>,
    pub free_bytes_after: Option<i64>,
    pub low_bytes: Option<i64>,
    pub target_bytes: Option<i64>,
    pub pressure_active: Option<bool>,
    pub hf: CleanerReport,
    pub weles: CleanerReport,
    pub builds: CleanerReport,
    pub clones: CleanerReport,
    pub caps: Caps,
    pub lock_busy: bool,
    pub active_slot_count: i64,
    pub last_success_at: Option<String>,
    pub errors: Vec<String>,
}

impl CleanupReport {
    pub fn base(active_slot_count: i64, hostname: &str) -> Self {
        Self {
            hostname: targets::normalize_hostname(hostname),
            target_name: None,
            policy_digest: None,
            mode: None,
            check_interval_seconds: None,
            started_at: utc_now(),
            duration_ms: 0,
            outcome: "invalid_or_unavailable_policy".to_string(),
            free_bytes_before: None,
            free_bytes_after: None,
            low_bytes: None,
            target_bytes: None,
            pressure_active: None,
            hf: CleanerReport::default(),
            weles: CleanerReport::default(),
            builds: CleanerReport::default(),
            clones: CleanerReport::default(),
            caps: Caps::default(),
            lock_busy: false,
            active_slot_count: active_slot_count.max(0),
            last_success_at: None,
            errors: Vec::new(),
        }
    }

    /// Python `_add_error`.
    pub fn add_error(&mut self, area: &str, exc: &JanitorError) {
        if self.errors.len() < MAX_ERRORS {
            self.errors.push(format!("{area}:{}", exc.error_code()));
        }
    }

    /// Python `_skip` for the HF cleaner.
    pub fn skip_hf(&mut self, reason: &str, count: i64) {
        *self.hf.skipped.entry(reason.to_string()).or_insert(0) += count;
    }

    /// Python `_skip` for the weles cleaner.
    pub fn skip_weles(&mut self, reason: &str, count: i64) {
        *self.weles.skipped.entry(reason.to_string()).or_insert(0) += count;
    }

    /// `_skip` for the build-cache cleaner (no Python original).
    pub fn skip_builds(&mut self, reason: &str, count: i64) {
        *self.builds.skipped.entry(reason.to_string()).or_insert(0) += count;
    }

    /// `_skip` for the Chromium clone cleaner (no Python original).
    pub fn skip_clones(&mut self, reason: &str, count: i64) {
        *self.clones.skipped.entry(reason.to_string()).or_insert(0) += count;
    }

    /// The report as JSON (key order normalized at serialization sites
    /// with [`canonical_json`], matching Python `json.dumps(sort_keys=True)`).
    pub fn to_value(&self) -> Value {
        let cleaner = |c: &CleanerReport| {
            serde_json::json!({
                "scanned_items": c.scanned_items,
                "eligible_items": c.eligible_items,
                "deleted_items": c.deleted_items,
                "expected_bytes": c.expected_bytes,
                "actual_free_delta_bytes": c.actual_free_delta_bytes,
                "skipped": c.skipped,
            })
        };
        serde_json::json!({
            "version": STATE_VERSION,
            "hostname": self.hostname,
            "target_name": self.target_name,
            "policy_digest": self.policy_digest,
            "mode": self.mode,
            "check_interval_seconds": self.check_interval_seconds,
            "started_at": self.started_at,
            "duration_ms": self.duration_ms,
            "outcome": self.outcome,
            "free_bytes_before": self.free_bytes_before,
            "free_bytes_after": self.free_bytes_after,
            "low_bytes": self.low_bytes,
            "target_bytes": self.target_bytes,
            "pressure_active": self.pressure_active,
            "cleaners": {
                "huggingface_cache": cleaner(&self.hf),
                "weles_recordings": cleaner(&self.weles),
                "build_caches": cleaner(&self.builds),
                chromium_clones::CLEANER: cleaner(&self.clones),
            },
            "caps": {
                "bytes": self.caps.bytes,
                "items": self.caps.items,
                "scan": self.caps.scan,
                "deadline": self.caps.deadline,
            },
            "lock_busy": self.lock_busy,
            "active_slot_count": self.active_slot_count,
            "last_success_at": self.last_success_at,
            "errors": self.errors,
        })
    }
}

/// Python `_utc_now` (`datetime.now(timezone.utc).isoformat()`).
fn utc_now() -> String {
    crate::models::isoformat_utc(chrono::Utc::now())
}

/// Python `time.time()` as f64 seconds.
fn epoch_now() -> f64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

// ---------------------------------------------------------------------------
// canonical JSON (Python json.dumps(sort_keys=True, separators=(",", ":")))
// ---------------------------------------------------------------------------

/// Serialize with recursively sorted object keys and compact separators —
/// byte-compatible with Python's `json.dumps(value, sort_keys=True,
/// separators=(",", ":"))` for the values the janitor emits (ASCII-safe;
/// non-ASCII strings are escaped like Python's default ensure_ascii=True).
pub fn canonical_json(value: &Value) -> String {
    let mut out = String::new();
    write_canonical(value, &mut out);
    out
}

fn write_canonical(value: &Value, out: &mut String) {
    match value {
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            out.push('{');
            for (index, key) in keys.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                write_canonical(&Value::String((*key).clone()), out);
                out.push(':');
                write_canonical(&map[*key], out);
            }
            out.push('}');
        }
        Value::Array(items) => {
            out.push('[');
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                write_canonical(item, out);
            }
            out.push(']');
        }
        // serde_json's own serializer matches json.dumps for scalars;
        // ensure_ascii escaping keeps Python parity for strings.
        other => out.push_str(&crate::models::ensure_ascii(
            &serde_json::to_string(other).unwrap_or_else(|_| "null".to_string()),
        )),
    }
}

// ---------------------------------------------------------------------------
// secure home / state dir / lock file
// ---------------------------------------------------------------------------

/// Python `_secure_home`: the home must be a real (non-symlink) directory
/// owned by the effective uid; returned fully resolved.
pub fn secure_home(home: &Path) -> Result<PathBuf, JanitorError> {
    let info = std::fs::symlink_metadata(home)?;
    if info.file_type().is_symlink() || !info.is_dir() {
        return Err(JanitorError::os("unsafe home"));
    }
    if info.uid() != euid() {
        return Err(JanitorError::os("home owner mismatch"));
    }
    Ok(std::fs::canonicalize(home)?)
}

/// The process effective uid (Python `os.geteuid()`).
pub(crate) fn euid() -> u32 {
    // SAFETY: geteuid cannot fail.
    unsafe { nix::libc::geteuid() }
}

/// Python `_ensure_state_dir`: create `~/.cache/wisent-compute` component
/// by component (mode 0700), refusing symlinks and foreign owners.
pub fn ensure_state_dir(home: &Path) -> Result<PathBuf, JanitorError> {
    let mut current = home.to_path_buf();
    for component in STATE_DIR_PARTS {
        current = current.join(component);
        let info = match std::fs::symlink_metadata(&current) {
            Ok(info) => info,
            Err(exc) if exc.kind() == io::ErrorKind::NotFound => {
                std::fs::create_dir(&current)?;
                // Match Python's mkdir(mode=0o700) exactly: umask may have
                // widened nothing, but be explicit like fchmod would be.
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&current, std::fs::Permissions::from_mode(0o700))?;
                std::fs::symlink_metadata(&current)?
            }
            Err(exc) => return Err(exc.into()),
        };
        if info.file_type().is_symlink() || !info.is_dir() {
            return Err(JanitorError::os("unsafe state directory"));
        }
        if info.uid() != euid() {
            return Err(JanitorError::os("state directory owner mismatch"));
        }
    }
    Ok(current)
}

/// Python `_open_lock`: open `disk-cleanup.lock` with O_RDWR|O_CREAT|
/// O_NOFOLLOW, verify it is a regular file owned by us, force 0600.
fn open_lock(state_dir: &Path) -> Result<File, JanitorError> {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .custom_flags(nix::libc::O_NOFOLLOW)
        .mode(0o600)
        .open(state_dir.join(LOCK_NAME))?;
    let info = file.metadata()?;
    if !info.is_file() || info.uid() != euid() {
        return Err(JanitorError::os("unsafe cleanup lock"));
    }
    use std::os::unix::fs::PermissionsExt;
    file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    Ok(file)
}

/// Python's `exc.errno in (errno.EACCES, errno.EAGAIN)` busy test (fs2
/// reports flock contention as WouldBlock; the raw-code check keeps the
/// Python errno set exactly).
fn lock_contended(exc: &io::Error) -> bool {
    exc.kind() == io::ErrorKind::WouldBlock
        || matches!(exc.raw_os_error(), Some(c) if c == nix::libc::EACCES || c == nix::libc::EAGAIN)
}

/// An exclusive cleanup-run lock that always issues `LOCK_UN` before close.
///
/// Explicit unlock is required for consistent semantics on macOS, where
/// closing one descriptor is not a sufficient release boundary when the
/// process has opened the same lock file more than once.
struct ExclusiveLock {
    file: File,
}

impl Drop for ExclusiveLock {
    fn drop(&mut self) {
        let _ = fs2::FileExt::unlock(&self.file);
    }
}

/// Python `_acquire_lock`: exclusive non-blocking flock; None when busy.
fn acquire_lock(state_dir: &Path) -> Result<Option<ExclusiveLock>, JanitorError> {
    let file = open_lock(state_dir)?;
    match fs2::FileExt::try_lock_exclusive(&file) {
        Ok(()) => Ok(Some(ExclusiveLock { file })),
        Err(exc) if lock_contended(&exc) => Ok(None),
        Err(exc) => Err(exc.into()),
    }
}

/// A shared-mode hold on the cleanup lock for one live workload
/// (Python's opaque `int` handle from `acquire_workload_lock`).
#[derive(Debug)]
pub struct WorkloadLock {
    file: Option<File>,
}

impl Drop for WorkloadLock {
    fn drop(&mut self) {
        if let Some(file) = self.file.take() {
            let _ = fs2::FileExt::unlock(&file);
        }
    }
}

/// Python `acquire_workload_lock` at an explicit home (test seam).
pub fn acquire_workload_lock_in(home: &Path) -> Result<Option<WorkloadLock>, JanitorError> {
    let state_dir = ensure_state_dir(&secure_home(home)?)?;
    let file = open_lock(&state_dir)?;
    match fs2::FileExt::try_lock_shared(&file) {
        Ok(()) => Ok(Some(WorkloadLock { file: Some(file) })),
        Err(exc) if lock_contended(&exc) => Ok(None),
        Err(exc) => Err(exc.into()),
    }
}

/// Acquire the cleanup lock in shared mode for one live workload.
///
/// The returned opaque handle must be retained until the workload has
/// fully left its slot, then passed to [`release_workload_lock`]. `None`
/// means a standalone cleanup currently owns the exclusive lock, so
/// admission must be retried later.
pub fn acquire_workload_lock() -> Result<Option<WorkloadLock>, JanitorError> {
    acquire_workload_lock_in(&crate::config_file::expand_tilde("~"))
}

/// Release a handle returned by [`acquire_workload_lock`]
/// (Python `release_workload_lock`: flock UN, then close; Drop provides the
/// same explicit unlock backstop when a caller releases by scope).
pub fn release_workload_lock(mut lock: WorkloadLock, log_fn: &mut dyn FnMut(&str)) {
    let Some(file) = lock.file.take() else {
        return;
    };
    if let Err(exc) = fs2::FileExt::unlock(&file) {
        log_fn(&format!(
            "disk cleanup workload lock release failed: {}",
            io_code(&exc)
        ));
    }
}

/// Map an io error to the Python `type(exc).__name__` style the agent logs.
fn io_code(exc: &io::Error) -> &'static str {
    JanitorError::from(io::Error::from_raw_os_error(
        exc.raw_os_error().unwrap_or(0),
    ))
    .code
}

// ---------------------------------------------------------------------------
// state file read / write
// ---------------------------------------------------------------------------

/// Python `_read_state`: owner-controlled, no-follow, plain-dict JSON.
fn read_state(state_dir: &Path) -> Result<Value, JanitorError> {
    let path = state_dir.join(STATE_NAME);
    let file = match OpenOptions::new()
        .read(true)
        .custom_flags(nix::libc::O_NOFOLLOW)
        .open(&path)
    {
        Ok(file) => file,
        Err(exc) if exc.kind() == io::ErrorKind::NotFound => return Ok(Value::Object(Map::new())),
        Err(exc) => return Err(exc.into()),
    };
    let info = file.metadata()?;
    if !info.is_file() || info.uid() != euid() {
        return Err(JanitorError::os("unsafe cleanup state"));
    }
    let mut text = String::new();
    (&file).read_to_string(&mut text)?;
    let value: Value = serde_json::from_str(&text)?;
    Ok(if value.is_object() {
        value
    } else {
        Value::Object(Map::new())
    })
}

/// Python `_write_state`: lstat the destination (refuse symlink / foreign
/// owner), write to a sibling tempfile (O_EXCL, 0600), fsync, atomic
/// rename, fsync the directory.
fn write_state(state_dir: &Path, report: &Value, attempted_at: f64) -> Result<(), JanitorError> {
    let destination = state_dir.join(STATE_NAME);
    match std::fs::symlink_metadata(&destination) {
        Ok(existing) => {
            if existing.file_type().is_symlink() || !existing.is_file() || existing.uid() != euid()
            {
                return Err(JanitorError::os("unsafe cleanup state"));
            }
        }
        Err(exc) if exc.kind() == io::ErrorKind::NotFound => {}
        Err(exc) => return Err(exc.into()),
    }
    let payload = canonical_json(&serde_json::json!({
        "version": STATE_VERSION,
        "last_attempt_at": attempted_at,
        "report": report,
    }));
    // Tempfile uniqueness like Python's f".{name}.{getpid()}.{monotonic_ns()}".
    let nanos = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let temp = state_dir.join(format!(".{STATE_NAME}.{}.{nanos}", std::process::id()));
    let result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .custom_flags(nix::libc::O_NOFOLLOW)
            .mode(0o600)
            .open(&temp)?;
        file.write_all(payload.as_bytes())?;
        file.flush()?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(&temp, &destination)?;
        let dir_fd = safefs::open_dir_path(state_dir)?;
        safefs::fsync(dir_fd.as_raw_fd())?;
        Ok(())
    })();
    let _ = std::fs::remove_file(&temp);
    result
}

// ---------------------------------------------------------------------------
// sanitized public report (Python `_sanitize_report` and helpers)
// ---------------------------------------------------------------------------

/// Python `_PUBLIC_OUTCOMES`.
const PUBLIC_OUTCOMES: [&str; 11] = [
    "never_run",
    "invalid_or_unavailable_policy",
    "lock_busy",
    "interval_noop",
    "healthy_noop",
    "report_only",
    "blocked_active",
    "reclaimed_target",
    "cap_reached",
    "partial_error",
    "no_eligible_items",
];

/// Python `_PUBLIC_SKIP_REASONS`. NOTE: the weles-internal reasons
/// `active_run`, `escapes_root`, and `item_cap` are deliberately absent
/// (they never leave the host), exactly as in the Python source.
const PUBLIC_SKIP_REASONS: [&str; 16] = [
    "active_slots",
    "blob_link_count_uncertain",
    "byte_cap",
    "cache_locked",
    "incomplete_repository",
    "lock_root_absent",
    "not_run_directory",
    "reserved_or_hidden",
    "root_absent",
    "root_changed",
    "scan_cap",
    "scan_deadline",
    "stat_failed",
    "too_young",
    "unsafe_owner_or_device",
    "upload_proof_unavailable_v1",
];

/// Python `_public_nonnegative`: ints only (never bools), floored at 0.
fn public_nonnegative(value: Option<&Value>) -> Option<i64> {
    match value {
        Some(Value::Number(n)) => n.as_i64().map(|v| v.max(0)),
        _ => None,
    }
}

fn public_cleaner(value: Option<&Value>) -> Value {
    let source = value.and_then(Value::as_object);
    let get = |key: &str| source.and_then(|map| map.get(key));
    let mut skipped = Map::new();
    if let Some(skipped_source) = get("skipped").and_then(Value::as_object) {
        for reason in PUBLIC_SKIP_REASONS {
            if let Some(count) = public_nonnegative(skipped_source.get(reason)) {
                if count != 0 {
                    skipped.insert(reason.to_string(), Value::from(count));
                }
            }
        }
    }
    serde_json::json!({
        "scanned_items": public_nonnegative(get("scanned_items")).unwrap_or(0),
        "eligible_items": public_nonnegative(get("eligible_items")).unwrap_or(0),
        "deleted_items": public_nonnegative(get("deleted_items")).unwrap_or(0),
        "expected_bytes": public_nonnegative(get("expected_bytes")).unwrap_or(0),
        "actual_free_delta_bytes": public_nonnegative(get("actual_free_delta_bytes")).unwrap_or(0),
        "skipped": Value::Object(skipped),
    })
}

/// Python `_public_timestamp`: bounded ISO-8601 strings only; returns the
/// re-serialized parse (or None).
fn public_timestamp(value: Option<&Value>) -> Option<String> {
    let text = match value {
        Some(Value::String(s)) if s.len() <= 48 => s,
        _ => return None,
    };
    parse_isoformat(text)
}

/// Python `datetime.fromisoformat(value.replace("Z", "+00:00"))` followed
/// by `.isoformat()`: accept the aware forms we emit plus naive forms,
/// reject everything else.
fn parse_isoformat(text: &str) -> Option<String> {
    let replaced = text.replace('Z', "+00:00");
    if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(&replaced) {
        let micros = dt.timestamp_subsec_micros();
        if micros == 0 {
            return Some(dt.format("%Y-%m-%dT%H:%M:%S%:z").to_string());
        }
        return Some(dt.format("%Y-%m-%dT%H:%M:%S%.6f%:z").to_string());
    }
    for fmt in [
        "%Y-%m-%dT%H:%M:%S%.f",
        "%Y-%m-%d %H:%M:%S%.f",
        "%Y-%m-%dT%H:%M:%S",
        "%Y-%m-%d %H:%M:%S",
    ] {
        if let Ok(dt) = chrono::NaiveDateTime::parse_from_str(text, fmt) {
            let micros = dt.and_utc().timestamp_subsec_micros();
            if micros == 0 {
                return Some(dt.format("%Y-%m-%dT%H:%M:%S").to_string());
            }
            return Some(dt.format("%Y-%m-%dT%H:%M:%S%.6f").to_string());
        }
    }
    None
}

/// Return the stable public report without host, path, or policy identity
/// data. Python `_sanitize_report`.
pub fn sanitize_report(value: &Value, lock_busy: bool) -> Value {
    let source = value.as_object();
    let get = |key: &str| source.and_then(|map| map.get(key));
    let cleaners = get("cleaners").and_then(Value::as_object);
    let caps = get("caps").and_then(Value::as_object);
    let mut safe_errors = Vec::new();
    if let Some(errors) = get("errors").and_then(Value::as_array) {
        for item in errors.iter().take(MAX_ERRORS) {
            if let Some(item) = item.as_str() {
                if !item.is_empty() && item.len() <= 128 && is_safe_error(item) {
                    safe_errors.push(Value::from(item));
                }
            }
        }
    }
    let outcome_raw = get("outcome").and_then(Value::as_str).unwrap_or("");
    let outcome = if lock_busy {
        "lock_busy"
    } else if PUBLIC_OUTCOMES.contains(&outcome_raw) {
        outcome_raw
    } else {
        "never_run"
    };
    let mode = match get("mode").and_then(Value::as_str) {
        Some(m @ ("off" | "report" | "enforce")) => Some(m),
        _ => None,
    };
    let cap = |name: &str| caps.and_then(|c| c.get(name)) == Some(&Value::Bool(true));
    serde_json::json!({
        "version": STATE_VERSION,
        "mode": mode,
        "check_interval_seconds": public_nonnegative(get("check_interval_seconds")),
        "started_at": public_timestamp(get("started_at")),
        "duration_ms": public_nonnegative(get("duration_ms")).unwrap_or(0),
        "outcome": outcome,
        "free_bytes_before": public_nonnegative(get("free_bytes_before")),
        "free_bytes_after": public_nonnegative(get("free_bytes_after")),
        "low_bytes": public_nonnegative(get("low_bytes")),
        "target_bytes": public_nonnegative(get("target_bytes")),
        "pressure_active": get("pressure_active").and_then(Value::as_bool),
        "cleaners": {
            "huggingface_cache": public_cleaner(cleaners.and_then(|c| c.get("huggingface_cache"))),
            "weles_recordings": public_cleaner(cleaners.and_then(|c| c.get("weles_recordings"))),
            "build_caches": public_cleaner(cleaners.and_then(|c| c.get("build_caches"))),
            chromium_clones::CLEANER: public_cleaner(
                cleaners.and_then(|c| c.get(chromium_clones::CLEANER)),
            ),
        },
        "caps": {
            "bytes": cap("bytes"),
            "items": cap("items"),
            "scan": cap("scan"),
            "deadline": cap("deadline"),
        },
        "lock_busy": lock_busy || get("lock_busy") == Some(&Value::Bool(true)),
        "active_slot_count": public_nonnegative(get("active_slot_count")).unwrap_or(0),
        "last_success_at": public_timestamp(get("last_success_at")),
        "errors": safe_errors,
    })
}

/// Python's `area, sep, code = item.partition(":")` + the alnum checks
/// (underscore-stripped; empty remainder is not alnum).
fn is_safe_error(item: &str) -> bool {
    let Some((area, code)) = item.split_once(':') else {
        return false;
    };
    let alnum = |s: &str| {
        let stripped: String = s.chars().filter(|c| *c != '_').collect();
        !stripped.is_empty() && stripped.chars().all(char::is_alphanumeric)
    };
    alnum(area) && alnum(code)
}

/// Return the path- and identity-free public form of a cleanup report.
/// Python `sanitize_cleanup_report`.
pub fn sanitize_cleanup_report(report: &Value) -> Value {
    sanitize_report(report, false)
}

/// Read the owner-controlled state under a shared, no-follow-safe lock.
/// Python `read_cleanup_state`.
pub fn read_cleanup_state_in(home: &Path) -> Result<Value, JanitorError> {
    let home = secure_home(home)?;
    let state_dir = ensure_state_dir(&home)?;
    let lock = open_lock(&state_dir)?;
    match fs2::FileExt::try_lock_shared(&lock) {
        Ok(()) => {}
        Err(exc) if lock_contended(&exc) => {
            return Ok(sanitize_report(&Value::Object(Map::new()), true));
        }
        Err(exc) => return Err(exc.into()),
    }
    let state = read_state(&state_dir)?;
    let report = state.get("report").cloned().unwrap_or(Value::Null);
    Ok(sanitize_report(&report, false))
}

/// Python `read_cleanup_state` at the real home.
pub fn read_cleanup_state() -> Result<Value, JanitorError> {
    read_cleanup_state_in(&crate::config_file::expand_tilde("~"))
}

// ---------------------------------------------------------------------------
// fixed roots + free space
// ---------------------------------------------------------------------------

/// Python `_fixed_root`: walk `parts` beneath `home`, requiring every
/// component to be a non-symlink directory owned by us on home's device;
/// the resolved root must stay strictly beneath home.
pub fn fixed_root(
    home: &Path,
    parts: &[OsString],
    required: bool,
) -> Result<Option<PathBuf>, JanitorError> {
    let mut current = home.to_path_buf();
    let home_device = std::fs::metadata(home)?.dev();
    for part in parts {
        current = current.join(part);
        let info = match std::fs::symlink_metadata(&current) {
            Ok(info) => info,
            Err(exc) if exc.kind() == io::ErrorKind::NotFound => {
                if required {
                    return Err(JanitorError::from(exc));
                }
                return Ok(None);
            }
            Err(exc) => return Err(exc.into()),
        };
        if info.file_type().is_symlink() || !info.is_dir() {
            return Err(JanitorError::os("unsafe cleaner root"));
        }
        if info.uid() != euid() || info.dev() != home_device {
            return Err(JanitorError::os(
                "cleaner root ownership or device mismatch",
            ));
        }
    }
    let resolved = std::fs::canonicalize(&current)?;
    if resolved == home || !resolved.starts_with(home) {
        return Err(JanitorError::os("cleaner root is not beneath home"));
    }
    Ok(Some(resolved))
}

/// Python `_free_bytes` (`shutil.disk_usage(home).free`).
pub fn free_bytes(home: &Path) -> Result<i64, JanitorError> {
    let stat = nix::sys::statvfs::statvfs(home)
        .map_err(|e| JanitorError::from(io::Error::from_raw_os_error(e as i32)))?;
    Ok((stat.blocks_available() as i64) * (stat.fragment_size() as i64))
}

// ---------------------------------------------------------------------------
// canonical policy resolution
// ---------------------------------------------------------------------------

/// Python `_fetch_canonical_registry`: fetch the canonical object
/// directly; destructive checks never use fallback/cache. Generation-
/// pinned via the store's versioned read (reload + pinned download, with
/// the same 412-retry the Python SDK path relies on).
///
/// DEVIATION from Python, matching `targets::download_registry_blob`: the
/// object is resolved by [`targets::RegistryStore`] instead of a
/// hardcoded GCS bucket. Pinned to GCS, the cleaner failed closed on an
/// Azure-only deployment — it could not read the policy that authorizes
/// deletion even though the dashboard was writing that policy to the
/// configured store. The "gcs" path is byte-identical.
async fn fetch_canonical_registry() -> Result<Value, JanitorError> {
    let store = targets::RegistryStore::open().await?;
    let text = store
        .read_versioned()
        .await?
        .ok_or_else(|| JanitorError::os("canonical registry generation unavailable"))?;
    let value: Value = serde_json::from_str(&text.content)?;
    if !value.is_object() {
        return Err(JanitorError::value("canonical registry is not an object"));
    }
    Ok(value)
}

/// The identity set of one raw registry target (Python `_identities`).
fn raw_identities(target: &Map<String, Value>) -> Vec<String> {
    let mut values = vec![targets::normalize_hostname(
        target.get("name").and_then(Value::as_str).unwrap_or(""),
    )];
    if let Some(hostnames) = target.get("hostnames").and_then(Value::as_array) {
        for value in hostnames {
            if let Some(value) = value.as_str() {
                values.push(targets::normalize_hostname(value));
            }
        }
    }
    if let Some(ssh) = target.get("ssh").and_then(Value::as_str) {
        if !ssh.is_empty() {
            values.push(targets::ssh_hostname(ssh));
        }
    }
    values
}

/// Resolve the unique local policy from validated canonical data.
///
/// No package registry fallback is permitted. Any fetch, schema,
/// identity, or typing failure is propagated to the caller, which must
/// fail closed. Python `resolve_canonical_policy`.
pub fn resolve_canonical_policy(
    data: &Value,
    hostname: &str,
) -> Result<(ComputeTarget, DiskCleanupPolicy, String), JanitorError> {
    targets::validate_registry(data).map_err(|exc| JanitorError::value(&exc.to_string()))?;
    let identity = targets::normalize_hostname(hostname);
    let targets_arr = data
        .get("targets")
        .and_then(Value::as_array)
        .ok_or_else(|| JanitorError::value("canonical registry has no targets"))?;
    let matches: Vec<&Map<String, Value>> = targets_arr
        .iter()
        .filter_map(Value::as_object)
        .filter(|raw| raw_identities(raw).contains(&identity))
        .collect();
    if matches.len() != 1 {
        return Err(JanitorError::lookup(
            "canonical host identity did not match uniquely",
        ));
    }
    let raw = matches[0];
    if raw.get("kind").and_then(Value::as_str) != Some("local") || !raw.contains_key("disk_cleanup")
    {
        return Err(JanitorError::lookup(
            "matched target has no local cleanup policy",
        ));
    }
    let target: ComputeTarget = serde_json::from_value(Value::Object(raw.clone()))
        .map_err(|_| JanitorError::lookup("cleanup policy could not be parsed"))?;
    let Some(policy) = target.disk_cleanup.clone() else {
        return Err(JanitorError::lookup("cleanup policy could not be parsed"));
    };
    let canonical = canonical_json(&raw["disk_cleanup"]);
    let digest = format!("{:x}", Sha256::digest(canonical.as_bytes()));
    Ok((target, policy, digest))
}

// ---------------------------------------------------------------------------
// run_cleanup_once
// ---------------------------------------------------------------------------

/// Shared scan budget + deadline (Python's `budget` dict + `deadline`).
pub struct ScanBudget {
    pub remaining: i64,
    pub deadline: Instant,
}

impl ScanBudget {
    pub fn new(max_scan_items: i64, deadline: Instant) -> Self {
        Self {
            remaining: max_scan_items,
            deadline,
        }
    }

    /// Python `_hf_tick`.
    pub fn tick(&mut self, report: &mut CleanupReport) -> Result<(), JanitorError> {
        if Instant::now() >= self.deadline {
            report.caps.deadline = true;
            return Err(JanitorError::timeout("cache scan deadline"));
        }
        if self.remaining <= 0 {
            report.caps.scan = true;
            return Err(JanitorError::os("cache scan cap"));
        }
        self.remaining -= 1;
        report.hf.scanned_items += 1;
        Ok(())
    }
}

/// Python `_finish`.
fn finish(
    mut report: CleanupReport,
    started: Instant,
    home: Option<&Path>,
    state_dir: Option<&Path>,
    attempted_at: f64,
    log_fn: &mut dyn FnMut(&str),
) -> Value {
    if let Some(home) = home {
        if let Ok(free) = free_bytes(home) {
            report.free_bytes_after = Some(free);
        }
    }
    report.duration_ms = (started.elapsed().as_secs_f64() * 1000.0).max(0.0) as i64;
    if let Some(state_dir) = state_dir {
        let value = report.to_value();
        if let Err(exc) = write_state(state_dir, &value, attempted_at) {
            report.add_error("state_write", &exc);
            if report.outcome != "lock_busy" && report.outcome != "invalid_or_unavailable_policy" {
                report.outcome = "partial_error".to_string();
            }
        }
    }
    report.errors.truncate(MAX_ERRORS);
    let value = report.to_value();
    let line = canonical_json(&value);
    // Python swallows logging failures.
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| log_fn(&line)));
    value
}

/// The post-lock half of `run_cleanup_once` (policy resolution through
/// outcome selection). Split out so tests can inject the canonical
/// registry document and a fabricated home without touching GCS or the
/// real `$HOME`. `_lock` holds the exclusive run lock until return
/// (Python's `finally: flock(LOCK_UN)`).
#[allow(clippy::too_many_arguments)]
fn run_with_lock(
    home: &Path,
    state_dir: &Path,
    _lock: ExclusiveLock,
    registry: Result<Value, JanitorError>,
    mut report: CleanupReport,
    started: Instant,
    attempted_at: f64,
    force: bool,
    // Plan only: pin an `enforce` policy down to the janitor's own
    // `report` mode for this pass, and persist nothing. See
    // `preview_cleanup_once`.
    preview: bool,
    log_fn: &mut dyn FnMut(&str),
) -> Value {
    // A preview leaves no trace. The state file is the janitor's record of
    // REAL passes: writing it would advance `last_attempt_at`, so an
    // operator asking what a cleanup WOULD delete would have silently
    // delayed the cleanup that does.
    let persist = if preview { None } else { Some(state_dir) };
    let (target, mut policy, digest) =
        match registry.and_then(|data| resolve_canonical_policy(&data, &report.hostname)) {
            Ok(value) => value,
            Err(exc) => {
                report.add_error("policy", &exc);
                return finish(report, started, Some(home), persist, attempted_at, log_fn);
            }
        };
    // `enforce` is the only mode that deletes. The janitor's own `report`
    // mode walks the identical scan and counts every eligible item without
    // unlinking one — `hf::run_hf` and `weles::scan_weles` both return
    // before their removal step whenever the mode is not `"enforce"` — so
    // a preview is this pass with that one word changed, not a second
    // implementation of the policy.
    //
    // `off` and `report` policies are left exactly as the registry states
    // them: a cleanup that is switched off would delete nothing, and the
    // preview has to say so rather than pretend the host is armed.
    if preview && policy.mode == "enforce" {
        policy.mode = "report".to_string();
    }
    report.target_name = Some(target.name);
    report.policy_digest = Some(digest.clone());
    report.mode = Some(policy.mode.clone());
    report.check_interval_seconds = Some(policy.check_interval_seconds);
    report.low_bytes = Some(policy.low_free_gb * GIB);
    report.target_bytes = Some(policy.target_free_gb * GIB);

    let previous = match read_state(state_dir) {
        Ok(value) => value,
        Err(exc) => {
            report.add_error("state_read", &exc);
            return finish(report, started, Some(home), persist, attempted_at, log_fn);
        }
    };
    let previous_report = previous.get("report").filter(|r| r.is_object()).cloned();
    report.last_success_at = previous_report
        .as_ref()
        .and_then(|r| r.get("last_success_at"))
        .and_then(|v| v.as_str().map(str::to_string));
    let last_attempt = previous.get("last_attempt_at").and_then(Value::as_f64);
    if !force
        && last_attempt.is_some()
        && attempted_at - last_attempt.unwrap_or(0.0) < policy.check_interval_seconds as f64
    {
        report.outcome = "interval_noop".to_string();
        return finish(
            report,
            started,
            Some(home),
            persist,
            last_attempt.unwrap_or(attempted_at),
            log_fn,
        );
    }

    let before = match free_bytes(home) {
        Ok(free) => free,
        Err(exc) => {
            report.add_error("runtime", &exc);
            report.outcome = "invalid_or_unavailable_policy".to_string();
            return finish(
                report,
                started,
                Some(home),
                Some(state_dir),
                attempted_at,
                log_fn,
            );
        }
    };
    report.free_bytes_before = Some(before);
    report.free_bytes_after = Some(before);
    let continuing_reclaim = previous_report
        .as_ref()
        .is_some_and(|r| r.get("policy_digest").and_then(Value::as_str) == Some(digest.as_str()))
        && previous_report
            .as_ref()
            .is_some_and(|r| r.get("outcome").and_then(Value::as_str) == Some("cap_reached"))
        && previous_report
            .as_ref()
            .is_some_and(|r| r.get("pressure_active") == Some(&Value::Bool(true)))
        && before < policy.target_free_gb * GIB;
    report.pressure_active = Some(before < policy.low_free_gb * GIB || continuing_reclaim);
    if policy.mode == "off" || report.pressure_active != Some(true) {
        report.outcome = "healthy_noop".to_string();
        report.last_success_at = Some(utc_now());
        return finish(report, started, Some(home), persist, attempted_at, log_fn);
    }

    let deadline = Instant::now() + std::time::Duration::from_secs_f64(DEADLINE_SECONDS);
    // Errors escaping _run_hf (a vanished cache root mid-pass, a failed
    // free-space probe) hit Python's outer `except BaseException`:
    // `runtime` error + the default outcome, state still written.
    if let Err(exc) = hf::run_hf(
        home,
        &policy,
        report.active_slot_count,
        attempted_at,
        deadline,
        &mut report,
    ) {
        report.add_error("runtime", &exc);
        report.outcome = "invalid_or_unavailable_policy".to_string();
        return finish(report, started, Some(home), persist, attempted_at, log_fn);
    }
    let scanned = report.hf.scanned_items;
    let remaining_scan = (policy.max_scan_items - scanned).max(0);
    if remaining_scan == 0 && policy.cleaners.contains_key("weles_recordings") {
        report.caps.scan = true;
    }
    weles::scan_weles(home, &policy, attempted_at, remaining_scan, &mut report);
    let remaining_after_weles =
        (policy.max_scan_items - report.hf.scanned_items - report.weles.scanned_items).max(0);
    if remaining_after_weles == 0 && policy.cleaners.contains_key("build_caches") {
        report.caps.scan = true;
    }
    // The build-cache scan is the only one whose root can be the whole of
    // `$HOME`: it walks with whatever scan budget the fixed-layout cleaners
    // left, and with the same pass deadline the HF scan honours.
    build_caches::scan_build_caches(
        home,
        &policy,
        attempted_at,
        remaining_after_weles,
        deadline,
        &mut report,
    );
    let remaining_after_builds = (policy.max_scan_items
        - report.hf.scanned_items
        - report.weles.scanned_items
        - report.builds.scanned_items)
        .max(0);
    if remaining_after_builds == 0 && policy.cleaners.contains_key(chromium_clones::CLEANER) {
        report.caps.scan = true;
    }
    // Last, and the only cleaner whose root is outside this account's home:
    // macOS puts the clones in the per-user temporary container, and what it
    // keeps there is nobody's working set — so it is scanned after every root
    // the fleet's own software writes to, on the budget those leave.
    chromium_clones::scan_chromium_clones(
        home,
        &policy,
        attempted_at,
        remaining_after_builds,
        deadline,
        &mut report,
    );
    let total_scanned = report.hf.scanned_items
        + report.weles.scanned_items
        + report.builds.scanned_items
        + report.clones.scanned_items;
    if total_scanned >= policy.max_scan_items {
        report.caps.scan = true;
    }

    let after = match free_bytes(home) {
        Ok(free) => free,
        Err(exc) => {
            report.add_error("runtime", &exc);
            report.outcome = "invalid_or_unavailable_policy".to_string();
            return finish(report, started, Some(home), persist, attempted_at, log_fn);
        }
    };
    report.free_bytes_after = Some(after);
    // Deliberately NOT `report.hf.deleted_items` alone, as the Python had
    // it: neither build_caches nor chromium_clones has a Python original to
    // stay faithful to, and a pass that removed 200 GB of tagged build trees
    // or 130 stale browser clones while the HF cache held nothing evictable
    // must not report `no_eligible_items`.
    let deleted =
        report.hf.deleted_items + report.builds.deleted_items + report.clones.deleted_items;
    if policy.mode != "enforce" {
        report.outcome = "report_only".to_string();
    } else if report.active_slot_count > 0 && policy.cleaners.contains_key("huggingface_cache") {
        report.outcome = "blocked_active".to_string();
    } else if after >= policy.target_free_gb * GIB {
        report.outcome = "reclaimed_target".to_string();
    } else if report.caps.any() {
        report.outcome = "cap_reached".to_string();
    } else if !report.errors.is_empty() {
        report.outcome = "partial_error".to_string();
    } else if deleted == 0 {
        report.outcome = "no_eligible_items".to_string();
    } else {
        report.outcome = "partial_error".to_string();
    }
    if report.errors.is_empty() {
        report.last_success_at = Some(utc_now());
    }
    finish(report, started, Some(home), persist, attempted_at, log_fn)
}

/// Resolve canonical policy and execute at most one bounded cleanup pass.
/// Python `run_cleanup_once`. Never fails: every failure mode lands in
/// the returned report (the agent mirrors Python's outcome handling).
pub async fn run_cleanup_once(
    active_slot_count: i64,
    force: bool,
    log_fn: &mut dyn FnMut(&str),
) -> Value {
    cleanup_once(active_slot_count, force, false, log_fn).await
}

/// Resolve canonical policy, plan one bounded pass, and delete NOTHING.
///
/// NO Python original: `stado/providers/local/disk/cleanup.py` has no
/// preview entry point. This is the same canonical policy, the same
/// exclusive lock, the same scanners and the same caps as
/// [`run_cleanup_once`] — the returned report's `eligible_items` and
/// `expected_bytes` per cleaner are what a real pass would remove right
/// now. Two differences, both documented at their site in
/// [`run_with_lock`]: an `enforce` policy is pinned down to the janitor's
/// own `report` mode for the duration, and no state is written.
///
/// The interval gate is bypassed, because an operator who asks what the
/// cleanup would delete must get an answer rather than `interval_noop`,
/// and no slots are declared, because the operator is not the agent and
/// holds none.
///
/// `stado disk-cleanup --dry-run` runs this locally;
/// `stado host cleanup TARGET --dry-run`
/// ([`crate::deploy::host_cleanup`]) runs it over ssh on the host being
/// previewed, which is the only place the host's filesystem exists.
pub async fn preview_cleanup_once(log_fn: &mut dyn FnMut(&str)) -> Value {
    cleanup_once(i64::default(), true, true, log_fn).await
}

/// The shared body of [`run_cleanup_once`] and [`preview_cleanup_once`].
async fn cleanup_once(
    active_slot_count: i64,
    force: bool,
    preview: bool,
    log_fn: &mut dyn FnMut(&str),
) -> Value {
    let started = Instant::now();
    let attempted_at = epoch_now();
    let hostname = crate::providers::vast::system_hostname();
    let mut report = CleanupReport::base(active_slot_count, &hostname);

    // Python's outer `except BaseException` half: any failure before the
    // policy resolves lands in `runtime` and leaves the default outcome.
    let home = match secure_home(&crate::config_file::expand_tilde("~")) {
        Ok(home) => home,
        Err(exc) => {
            report.add_error("runtime", &exc);
            report.outcome = "invalid_or_unavailable_policy".to_string();
            return finish(report, started, None, None, attempted_at, log_fn);
        }
    };
    let state_dir = match ensure_state_dir(&home) {
        Ok(dir) => dir,
        Err(exc) => {
            report.add_error("runtime", &exc);
            report.outcome = "invalid_or_unavailable_policy".to_string();
            return finish(report, started, Some(&home), None, attempted_at, log_fn);
        }
    };
    let persist = if preview {
        None
    } else {
        Some(state_dir.as_path())
    };
    let lock = match acquire_lock(&state_dir) {
        Ok(lock) => lock,
        Err(exc) => {
            report.add_error("runtime", &exc);
            report.outcome = "invalid_or_unavailable_policy".to_string();
            return finish(report, started, Some(&home), persist, attempted_at, log_fn);
        }
    };
    let Some(lock) = lock else {
        report.lock_busy = true;
        report.outcome = "lock_busy".to_string();
        return finish(report, started, Some(&home), None, attempted_at, log_fn);
    };
    let registry = fetch_canonical_registry().await;
    run_with_lock(
        &home,
        &state_dir,
        lock,
        registry,
        report,
        started,
        attempted_at,
        force,
        preview,
        log_fn,
    )
}

// ---------------------------------------------------------------------------
// agent-side low-watermark plumbing (Python local_agent helpers)
// ---------------------------------------------------------------------------

/// Python `_validated_report_low_bytes`: a threshold only from a
/// successfully resolved policy report.
pub fn validated_report_low_bytes(report: &Value) -> Option<i64> {
    let digest = report.get("policy_digest").and_then(Value::as_str)?;
    // Python `int(digest, 16)`: arbitrary precision, so any 64-char hex
    // string validates (including high-bit-set digests that overflow u128).
    if digest.len() != 64 || !digest.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    match report.get("low_bytes") {
        Some(Value::Number(n)) => n.as_i64().filter(|v| *v > 0),
        _ => None,
    }
}

/// Read the last canonical low watermark from janitor-owned safe state
/// (Python `_persisted_disk_low_bytes` at an explicit home; test seam).
///
/// Reuse a threshold only when it came from the janitor's
/// owner-controlled, no-follow state file and the report identifies a
/// validated canonical policy.
pub fn persisted_disk_low_bytes_in(home: &Path) -> Option<i64> {
    let state_path = home.join(".cache").join("wisent-compute").join(STATE_NAME);
    for directory in [
        home.to_path_buf(),
        home.join(".cache"),
        home.join(".cache/wisent-compute"),
    ] {
        let info = std::fs::symlink_metadata(&directory).ok()?;
        if info.file_type().is_symlink() || !info.is_dir() || info.uid() != euid() {
            return None;
        }
    }
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(nix::libc::O_NOFOLLOW)
        .open(&state_path)
        .ok()?;
    let info = file.metadata().ok()?;
    if !info.is_file() || info.uid() != euid() || info.len() > 1024 * 1024 {
        return None;
    }
    let mut text = String::new();
    (&file).read_to_string(&mut text).ok()?;
    let state: Value = serde_json::from_str(&text).ok()?;
    if state.get("version")?.as_i64()? != STATE_VERSION {
        return None;
    }
    validated_report_low_bytes(state.get("report")?)
}

/// Python `_persisted_disk_low_bytes` at the real home.
pub fn persisted_disk_low_bytes() -> Option<i64> {
    persisted_disk_low_bytes_in(&crate::config_file::expand_tilde("~"))
}

/// Fail admission closed until both policy threshold and free space are
/// known. Python `_disk_pressure_unresolved`.
pub fn disk_pressure_unresolved(low_bytes: Option<i64>, free_bytes: Option<i64>) -> bool {
    match (low_bytes, free_bytes) {
        (Some(low), Some(free)) => free < low,
        _ => true,
    }
}

