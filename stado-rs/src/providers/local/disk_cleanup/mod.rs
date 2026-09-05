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

pub mod backup_twins;
pub mod build_caches;
pub mod chromium_clones;
pub mod hf;
pub mod queue_workdirs;
pub mod release_store;
pub mod safefs;
pub mod weles;

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::fs::{File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

use crate::constants;
use crate::targets::{self, ComputeTarget, DiskCleanupPolicy};

pub(crate) const GIB: i64 = 1024 * 1024 * 1024;
/// Python `_STATE_VERSION`.
pub const STATE_VERSION: i64 = 1;

/// Per-writer attempt stamps in the state file: `{writer: epoch_seconds}`.
///
/// A KEY and not a version bump, deliberately. `persisted_disk_low_bytes_in`
/// requires `version == STATE_VERSION` exactly, and that value feeds
/// `disk_pressure_unresolved`, which fails admission CLOSED when the low
/// watermark is unknown. Bumping the version would therefore make every
/// binary older than this one treat the state file as unreadable and stop
/// admitting work, on a fleet that demonstrably runs several versions at
/// once. An unknown key is ignored by those readers instead.
const WRITER_ATTEMPTS: &str = "last_attempt_by_writer";
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
/// Who holds the exclusive run lock, and until when they said they would.
///
/// `flock` states that somebody holds the lock and can state nothing else.
/// That was enough while every holder finished, and on 2026-09-03 it was not:
/// `charless-mac-mini` reported `disk_cleanup_stalled` for nine and a half
/// hours because one agent process held this lock, idle at 0% CPU with
/// eleven ESTABLISHED sockets to an object API whose pid no longer existed,
/// and a hold that never ends disables cleanup on the host permanently. The
/// kernel frees a dead holder's lock; it cannot free a live holder that will
/// never come back, and nothing in the file said the holder was overdue.
const LOCK_HOLDER_NAME: &str = "disk-cleanup.lock.holder";
/// How long past a holder's own declared deadline the lock may be taken over.
///
/// The criterion is deliberately NOT elapsed time alone: a long pass on a
/// large tree is healthy, and stealing its lock would produce exactly the
/// concurrent deletion the lock exists to prevent. It is the holder's OWN
/// promise — the pass deadline it recorded when it acquired the lock — plus
/// this grace. A holder past that has either stopped or lied about its
/// budget, and both are states nobody should have to wait out.
const LOCK_TAKEOVER_GRACE_S: f64 = 300.0;
/// Retired lock inodes remain linked under this prefix until their original
/// holder releases them. A replacement lock must never authorize deletion
/// while one of these files is still locked.
const RETIRED_LOCK_PREFIX: &str = "disk-cleanup.lock.retired.";
/// Serializes the short compare-and-replace sequence between takeover
/// contenders. It is never held while a cleanup pass runs.
const TAKEOVER_LOCK_NAME: &str = "disk-cleanup.lock.takeover";
/// Inode-specific holder records survive a legacy predecessor removing the
/// canonical holder pathname after its lock inode has been retired.
const LOCK_HOLDER_INODE_PREFIX: &str = "disk-cleanup.lock.holder.inode.";

/// How long one pass may wait on the queue store for its workdir keep-list.
///
/// NO Python original. Every other bound in this module — [`DEADLINE_SECONDS`],
/// `max_scan_items`, `max_items_per_pass` — governs work done AFTER the lock,
/// and the keep-list read is the only thing a pass waits on before it. It had
/// no bound at all, and the store's own HTTP client sets no timeout either
/// (`queue::gcs` builds a bare `reqwest::Client`), so a stalled listing
/// stalled the pass for as long as the transport took. On 2026-09-03
/// charless-mac-mini published `duration_ms: 818021` for a pass that reached
/// no cleaner.
///
/// Half of [`DEADLINE_SECONDS`], because the whole point of the janitor's own
/// default pass budget is that a pass is a short thing, and a keep-list read
/// that outlasts the scan it feeds is not a slow read but a broken one. The
/// expiry is not a failure: `None` is the keep-list's modelled unreadable
/// answer and [`queue_workdirs`] already refuses to delete on it and records
/// `queue_store_unreadable`.
const KEEP_LIST_BUDGET: Duration = Duration::from_secs(DEADLINE_SECONDS as u64 / 2);

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

/// The janitor's exclusive run lock relative to `$HOME`, exported for the
/// same reason as [`state_relative_path`].
///
/// `lock_busy` and the agent's `cleanup_in_progress` are two views of one
/// fact — somebody holds this file — and the product could print both
/// without ever naming the holder. On 2026-08-31 charless-mac-mini reported
/// them in alternation for hours while every cleaner scanned zero, and no
/// command in the fleet could say which process was holding it:
/// `host exec`'s allowlist has no form that names the owner of a file lock,
/// correctly, because an operator-supplied path there would be a hole. So
/// the path travels as a crate constant, and [`crate::deploy::host_disk`]
/// splices it into its own fixed remote program.
pub fn lock_relative_path() -> String {
    let mut parts: Vec<&str> = STATE_DIR_PARTS.to_vec();
    parts.push(LOCK_NAME);
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
    /// This build refused a WELL-FORMED registry: the document parsed, and
    /// declares something this binary has no implementation for.
    ///
    /// Distinct from [`JanitorError::value`] because the journal entry is the
    /// operator's only signal, and `policy:ValueError` says "the registry is
    /// invalid" — which was false for all 8348 refusals between
    /// 2026-08-20 and 2026-09-02. The registry was valid; the running process
    /// was older than it. Three build eras refused today's document for three
    /// different reasons (an unknown cleaner name, then a changed required
    /// field set), each indistinguishable in the journal from a corrupt file,
    /// and each cleared only by an unrelated restart onto a newer build.
    ///
    /// `NotImplementedError` keeps the file's convention — codes are Python
    /// exception TYPE NAMES, and this is the builtin Python raises for an
    /// operation the running code does not implement. It is also strictly
    /// more specific than the `RuntimeError` it derives from, which this
    /// crate already spends on HTTP and state-machine failures elsewhere.
    ///
    /// The distinction is the CODE, not the area. The area is the janitor's
    /// coarse lifecycle stage (`runtime` before policy resolves, `policy`
    /// once it is being resolved) and a refusal happens squarely inside
    /// policy resolution; the code is this file's "what kind of failure" axis
    /// throughout. Nothing but this error crosses the boundary out of
    /// [`resolve_canonical_policy`], so a new area would have to be carried
    /// on the error anyway — the same field, spelled less accurately.
    ///
    /// The message stays private, as it does for every other constructor:
    /// [`JanitorError::error_code`] records the type name alone, and a
    /// rejection sentence names field paths.
    pub fn unsupported(message: &str) -> Self {
        Self {
            code: "NotImplementedError",
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
    /// Which process made this pass, and the version of the binary that made
    /// it. The state file has several writers on an always-on host; see
    /// [`CleanupWriter`] for why attribution rather than arbitration.
    pub writer: &'static str,
    pub writer_version: &'static str,
    /// True when this host declares no `disk_cleanup` and the reporting
    /// default is in force. An operator reading `mode: report` otherwise
    /// cannot tell a deliberate choice from an absent declaration.
    pub policy_defaulted: bool,
    pub mode: Option<String>,
    pub check_interval_seconds: Option<i64>,
    pub started_at: String,
    pub duration_ms: i64,
    /// Of `duration_ms`, how much was spent waiting on the queue store before
    /// the pass had decided anything — the canonical-registry read and the
    /// workdir keep-list read, both of which happen before the run lock.
    ///
    /// `duration_ms` said 818021 and `outcome` said `healthy_noop`, and
    /// between those two numbers there was no way to tell a janitor that had
    /// walked a very large filesystem from one that had waited a quarter of an
    /// hour on a network read for a keep-list no cleaner on that pass would
    /// ever consult. Those call for opposite responses — one is the host, the
    /// other is ours — and every consumer of this report was being handed the
    /// verdict without the cost's shape. `scanned`/`cleaners: null` already
    /// says a pass reached no cleaner; this says where such a pass spent its
    /// time, so `healthy_noop` can never again hide a wait inside a word that
    /// means "nothing needed doing".
    pub store_wait_ms: i64,
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
    pub workdirs: CleanerReport,
    pub backup_twins: CleanerReport,
    pub release_store: CleanerReport,
    pub caps: Caps,
    pub lock_busy: bool,
    pub active_job_count: i64,
    pub last_success_at: Option<String>,
    /// Whether this pass reached its cleaners at all.
    ///
    /// Set once, immediately before the first cleaner runs. It exists because
    /// the report used to carry a complete cleaner table of zeros no matter
    /// how early the pass gave up, and a table of zeros is byte-for-byte what
    /// a successful pass that found nothing to delete emits. On
    /// `lukasz-macbook` that made 12,197 records over fifteen days — 8,539 of
    /// them `invalid_or_unavailable_policy` and 2,030 `lock_busy`, neither of
    /// which resolved a policy or opened a single directory — indistinguishable
    /// from fifteen days of "nothing needed doing", which is why nobody
    /// noticed the janitor had never once deleted anything.
    ///
    /// A pass that did not reach its cleaners now emits `cleaners: null`
    /// rather than a measurement it never made. Both readers of the table
    /// already tolerate its absence
    /// ([`crate::deploy::host_cleanup::cleaner_plans`] returns no rows and
    /// `stado host disk` prints no per-cleaner section), and the `outcome`
    /// vocabulary is unchanged: `lock_busy`, `interval_noop`,
    /// `invalid_or_unavailable_policy` and `healthy_noop` already say which
    /// non-run this was.
    pub scanned: bool,
    /// Declared cleaners this pass never scanned, because the scan share or
    /// the pass deadline was spent before their turn came.
    ///
    /// [`scanned`](Self::scanned) says whether a pass reached its cleaners at
    /// all; this says which of them it never reached, and it exists for the
    /// same reason: the table publishes `scanned 0, eligible 0, deleted 0`
    /// for a cleaner that was never given a turn, which is byte-for-byte what
    /// a cleaner that looked and found nothing emits. On `charless-mac-mini`
    /// the cleaners run in a fixed order with `backup_twins` last, the policy
    /// declared no `max_pass_seconds` so every pass took the janitor's own 30
    /// seconds against a `$HOME` holding 103.9 GiB under `~/.stado` alone,
    /// and `build_caches` — which walks all of `$HOME` by design — ended the
    /// pass inside itself. `backup_twins` reported zeros with
    /// `skipped {scan_cap: 1, scan_deadline: 1}` for as long as anyone had
    /// looked, under real pressure, while the host refused every ordinary job
    /// for eleven days. The outcome was `cap_reached`, which is true, names
    /// the budget and not the cleaner, and reads like a finished look at the
    /// disk.
    ///
    /// Empty when every declared cleaner had its turn, so a reader can tell
    /// "nothing was eligible" from "nobody looked".
    pub unscanned_cleaners: Vec<String>,
    /// Cleaners the policy names that this binary does not implement: the
    /// registry is read by every release at once, and a name a newer release
    /// knows is not a reason to run none of the ones this release knows.
    pub unknown_cleaners: Vec<String>,
    /// Where the build-cache walk stopped, relative to its scan root, or
    /// `None` when it crossed the whole tree. Carried across passes through
    /// the state file: see [`build_caches::scan_build_caches`].
    pub builds_resume_from: Option<String>,
    pub errors: Vec<String>,
}

impl CleanupReport {
    pub fn base(active_job_count: i64, hostname: &str) -> Self {
        Self {
            hostname: targets::normalize_hostname(hostname),
            target_name: None,
            policy_digest: None,
            writer: "unknown",
            writer_version: crate::build_identity::BUILD_IDENTITY,
            policy_defaulted: false,
            mode: None,
            check_interval_seconds: None,
            started_at: utc_now(),
            duration_ms: 0,
            store_wait_ms: 0,
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
            workdirs: CleanerReport::default(),
            backup_twins: CleanerReport::default(),
            release_store: CleanerReport::default(),
            caps: Caps::default(),
            lock_busy: false,
            active_job_count: active_job_count.max(0),
            last_success_at: None,
            scanned: false,
            unscanned_cleaners: Vec::new(),
            unknown_cleaners: Vec::new(),
            builds_resume_from: None,
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

    pub fn skip_workdirs(&mut self, reason: &str, count: i64) {
        *self.workdirs.skipped.entry(reason.to_string()).or_insert(0) += count;
    }

    pub fn skip_backup_twins(&mut self, reason: &str, count: i64) {
        *self
            .backup_twins
            .skipped
            .entry(reason.to_string())
            .or_insert(0) += count;
    }

    pub fn skip_release_store(&mut self, reason: &str, count: i64) {
        *self
            .release_store
            .skipped
            .entry(reason.to_string())
            .or_insert(0) += count;
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
        // A table of zeros and a table that was never filled in are the same
        // bytes, so a pass that did not reach its cleaners states the absence
        // instead. See [`CleanupReport::scanned`].
        let cleaners = if self.scanned {
            serde_json::json!({
                "huggingface_cache": cleaner(&self.hf),
                "weles_recordings": cleaner(&self.weles),
                "build_caches": cleaner(&self.builds),
                chromium_clones::CLEANER: cleaner(&self.clones),
                queue_workdirs::CLEANER: cleaner(&self.workdirs),
                backup_twins::CLEANER: cleaner(&self.backup_twins),
                release_store::CLEANER: cleaner(&self.release_store),
            })
        } else {
            Value::Null
        };
        serde_json::json!({
            "version": STATE_VERSION,
            "hostname": self.hostname,
            "target_name": self.target_name,
            "policy_digest": self.policy_digest,
            "writer": self.writer,
            "writer_version": self.writer_version,
            // The pid that wrote this pass. `writer` names WHICH entry point
            // ran and `writer_version` names what it was built from, and on
            // 2026-08-31 neither was enough: charless-mac-mini's state file
            // was overwritten every forty-five seconds by a build older than
            // the one that stamps those fields, so the file said nothing at
            // all about its author and no sampling caught the process alive.
            // A pid outlives the process in the file it wrote, which is the
            // whole difference between "somebody is writing this" and a name.
            "writer_pid": std::process::id(),
            "policy_defaulted": self.policy_defaulted,
            "mode": self.mode,
            "check_interval_seconds": self.check_interval_seconds,
            "started_at": self.started_at,
            "duration_ms": self.duration_ms,
            "store_wait_ms": self.store_wait_ms,
            "outcome": self.outcome,
            "free_bytes_before": self.free_bytes_before,
            "free_bytes_after": self.free_bytes_after,
            "low_bytes": self.low_bytes,
            "target_bytes": self.target_bytes,
            "pressure_active": self.pressure_active,
            "cleaners": cleaners,
            "unscanned_cleaners": self.unscanned_cleaners,
            "unknown_cleaners": self.unknown_cleaners,
            "caps": {
                "bytes": self.caps.bytes,
                "items": self.caps.items,
                "scan": self.caps.scan,
                "deadline": self.caps.deadline,
            },
            "lock_busy": self.lock_busy,
            "active_job_count": self.active_job_count,
            "last_success_at": self.last_success_at,
            "build_caches_resume_from": self.builds_resume_from,
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
    open_lock_at(&state_dir.join(LOCK_NAME))
}

/// The same checks at an exact path, for the takeover's staged file.
fn open_lock_at(path: &Path) -> Result<File, JanitorError> {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .custom_flags(nix::libc::O_NOFOLLOW)
        .mode(0o600)
        .open(path)?;
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
/// The holder token makes record removal conditional. Without it, a process
/// whose old lock inode was retired could finish later and delete the current
/// holder's record merely because both records use the same pathname.
struct ExclusiveLock {
    file: File,
    holder_records: Vec<PathBuf>,
    holder_token: Option<String>,
}

impl Drop for ExclusiveLock {
    fn drop(&mut self) {
        let _ = fs2::FileExt::unlock(&self.file);
        let Some(token) = &self.holder_token else {
            return;
        };
        for path in &self.holder_records {
            let current_token = read_lock_holder_at(path).map(|holder| holder.token);
            if current_token.as_deref() == Some(token.as_str()) {
                let _ = std::fs::remove_file(path);
            }
        }
    }
}

/// What one holder said about itself when it took the lock.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct LockHolder {
    pid: i32,
    acquired_at: f64,
    /// Epoch seconds by which this holder expects to be finished: its pass
    /// deadline, not a guess made by the reader.
    deadline_at: f64,
    writer: String,
    writer_version: String,
    /// Unique ownership token. Empty only for records written by an older
    /// Stado release; those remain readable during the rolling upgrade.
    #[serde(default)]
    token: String,
}

/// Why a contended lock is contended, in the terms an operator needs.
enum LockState {
    /// Ours, and the record now says so.
    Held(ExclusiveLock),
    /// Somebody else holds it and is still inside their declared budget.
    Busy { holder: Option<LockHolder> },
    /// Taken from a holder that is past its own declared deadline. Carries the
    /// evidence so the pass can report it rather than looking like a normal
    /// run.
    TakenOver {
        lock: ExclusiveLock,
        from_pid: i32,
        overdue_seconds: f64,
    },
}

fn holder_record_path(state_dir: &Path) -> PathBuf {
    state_dir.join(LOCK_HOLDER_NAME)
}

fn holder_inode_record_path(state_dir: &Path, file: &File) -> Result<PathBuf, JanitorError> {
    let info = file.metadata()?;
    Ok(state_dir.join(format!(
        "{LOCK_HOLDER_INODE_PREFIX}{}.{}",
        info.dev(),
        info.ino()
    )))
}

fn read_lock_holder_at(path: &Path) -> Option<LockHolder> {
    let mut file = OpenOptions::new()
        .read(true)
        .custom_flags(nix::libc::O_NOFOLLOW)
        .open(path)
        .ok()?;
    let info = file.metadata().ok()?;
    if !info.is_file() || info.uid() != euid() {
        return None;
    }
    let mut raw = String::new();
    file.read_to_string(&mut raw).ok()?;
    serde_json::from_str(&raw).ok()
}

fn read_lock_holder(state_dir: &Path, lock: &File) -> Option<LockHolder> {
    holder_inode_record_path(state_dir, lock)
        .ok()
        .and_then(|path| read_lock_holder_at(&path))
        .or_else(|| read_lock_holder_at(&holder_record_path(state_dir)))
}

fn lock_token() -> String {
    let nanos = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!("{}-{nanos}", std::process::id())
}

fn write_lock_holder(
    state_dir: &Path,
    lock: &File,
    pass_seconds: f64,
    writer: &str,
) -> Result<(String, Vec<PathBuf>), JanitorError> {
    let now = epoch_now();
    let token = lock_token();
    let record = LockHolder {
        pid: std::process::id() as i32,
        acquired_at: now,
        deadline_at: now + pass_seconds,
        writer: writer.to_string(),
        writer_version: env!("CARGO_PKG_VERSION").to_string(),
        token: token.clone(),
    };
    let body = serde_json::to_vec(&record)?;
    let records = vec![
        holder_inode_record_path(state_dir, lock)?,
        holder_record_path(state_dir),
    ];
    let mut written = Vec::new();
    for destination in &records {
        if let Ok(info) = std::fs::symlink_metadata(destination) {
            if info.file_type().is_symlink() || !info.is_file() || info.uid() != euid() {
                for path in &written {
                    let _ = std::fs::remove_file(path);
                }
                return Err(JanitorError::os("unsafe cleanup lock holder record"));
            }
        }
        let file_name = destination
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| JanitorError::os("invalid cleanup lock holder record path"))?;
        let staged = state_dir.join(format!(".{file_name}.{token}"));
        let result = (|| -> Result<(), JanitorError> {
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .custom_flags(nix::libc::O_NOFOLLOW)
                .mode(0o600)
                .open(&staged)?;
            file.write_all(&body)?;
            file.sync_data()?;
            std::fs::rename(&staged, destination)?;
            Ok(())
        })();
        if let Err(error) = result {
            let _ = std::fs::remove_file(&staged);
            for path in &written {
                let _ = std::fs::remove_file(path);
            }
            return Err(error);
        }
        written.push(destination.clone());
    }
    Ok((token, records))
}

/// Is a pid still a process on this host?
///
/// `kill(pid, 0)` answers exactly that and nothing else: `ESRCH` means gone,
/// `EPERM` means alive and owned by somebody else. A gone holder's `flock` is
/// already released by the kernel, so this is a diagnosis rather than a
/// release mechanism — it is what lets the report distinguish "the holder
/// died mid-pass" from "the holder is hung".
fn pid_alive(pid: i32) -> bool {
    if pid <= 0 {
        return false;
    }
    match unsafe { nix::libc::kill(pid, 0) } {
        0 => true,
        _ => io::Error::last_os_error().raw_os_error() == Some(nix::libc::EPERM),
    }
}

fn open_existing_lock_at(path: &Path) -> Result<File, JanitorError> {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags(nix::libc::O_NOFOLLOW)
        .open(path)?;
    let info = file.metadata()?;
    if !info.is_file() || info.uid() != euid() {
        return Err(JanitorError::os("unsafe cleanup lock file"));
    }
    Ok(file)
}

fn path_names_file(path: &Path, file: &File) -> bool {
    let Ok(path_info) = std::fs::symlink_metadata(path) else {
        return false;
    };
    let Ok(file_info) = file.metadata() else {
        return false;
    };
    !path_info.file_type().is_symlink()
        && path_info.is_file()
        && path_info.uid() == euid()
        && path_info.dev() == file_info.dev()
        && path_info.ino() == file_info.ino()
}

/// Check every retired predecessor inode while holding the current exclusive
/// lock. An unlocked predecessor is removed; a still-locked one keeps this
/// pass report-only so two lock generations can never authorize deletion at
/// the same time.
fn retired_locks_active(state_dir: &Path, current_lock: &File) -> Result<bool, JanitorError> {
    let current_info = current_lock.metadata()?;
    let mut active = false;
    for entry in std::fs::read_dir(state_dir)? {
        let entry = entry?;
        let name = entry.file_name();
        if !name.to_string_lossy().starts_with(RETIRED_LOCK_PREFIX) {
            continue;
        }
        let path = entry.path();
        let file = open_existing_lock_at(&path)?;
        let info = file.metadata()?;
        if info.dev() == current_info.dev() && info.ino() == current_info.ino() {
            // A contender can die after creating the hard link but before
            // replacing the canonical pathname. This process owns that same
            // inode exclusively, so the extra name is safe to remove.
            if path_names_file(&path, &file) {
                std::fs::remove_file(path)?;
            }
            continue;
        }
        match fs2::FileExt::try_lock_exclusive(&file) {
            Ok(()) => {
                let still_named = path_names_file(&path, &file);
                let stale_holder = holder_inode_record_path(state_dir, &file)?;
                fs2::FileExt::unlock(&file)?;
                if still_named {
                    std::fs::remove_file(path)?;
                    let _ = std::fs::remove_file(stale_holder);
                }
            }
            Err(error) if lock_contended(&error) => active = true,
            Err(error) => return Err(error.into()),
        }
    }
    Ok(active)
}

fn overdue_holder(state_dir: &Path, lock: &File) -> Option<(LockHolder, f64)> {
    let holder = read_lock_holder(state_dir, lock)?;
    let overdue_seconds = epoch_now() - (holder.deadline_at + LOCK_TAKEOVER_GRACE_S);
    (overdue_seconds > 0.0).then_some((holder, overdue_seconds))
}

/// Exclusive flock, with a bounded and mutually exclusive recovery path.
///
/// A takeover keeps a hard link to the predecessor inode before atomically
/// replacing the canonical pathname. Every later cleanup probes that retired
/// inode and remains report-only until its kernel lock is released. The short
/// hard-link/rename section has its own mutex, preventing two contenders from
/// retiring different generations concurrently.
fn acquire_lock_state(
    state_dir: &Path,
    pass_seconds: f64,
    writer: &str,
) -> Result<LockState, JanitorError> {
    let canonical = state_dir.join(LOCK_NAME);
    let file = open_lock(state_dir)?;
    match fs2::FileExt::try_lock_exclusive(&file) {
        Ok(()) => {
            let (token, records) = write_lock_holder(state_dir, &file, pass_seconds, writer)?;
            return Ok(LockState::Held(ExclusiveLock {
                file,
                holder_records: records,
                holder_token: Some(token),
            }));
        }
        Err(error) if lock_contended(&error) => {}
        Err(error) => return Err(error.into()),
    }
    let initial_holder = read_lock_holder(state_dir, &file);
    if overdue_holder(state_dir, &file).is_none() {
        return Ok(LockState::Busy {
            holder: initial_holder,
        });
    }

    let takeover_file = open_lock_at(&state_dir.join(TAKEOVER_LOCK_NAME))?;
    match fs2::FileExt::try_lock_exclusive(&takeover_file) {
        Ok(()) => {}
        Err(error) if lock_contended(&error) => {
            return Ok(LockState::Busy {
                holder: initial_holder,
            });
        }
        Err(error) => return Err(error.into()),
    }
    let _takeover_guard = ExclusiveLock {
        file: takeover_file,
        holder_records: Vec::new(),
        holder_token: None,
    };

    // Re-open and re-evaluate after winning the takeover mutex. Another
    // contender may have replaced or released the lock while we waited.
    let current = open_lock(state_dir)?;
    match fs2::FileExt::try_lock_exclusive(&current) {
        Ok(()) => {
            let (token, records) = write_lock_holder(state_dir, &current, pass_seconds, writer)?;
            return Ok(LockState::Held(ExclusiveLock {
                file: current,
                holder_records: records,
                holder_token: Some(token),
            }));
        }
        Err(error) if lock_contended(&error) => {}
        Err(error) => return Err(error.into()),
    }
    let Some((holder, overdue_seconds)) = overdue_holder(state_dir, &current) else {
        return Ok(LockState::Busy {
            holder: read_lock_holder(state_dir, &current),
        });
    };

    let replacement_token = lock_token();
    let current_info = current.metadata()?;
    let retired = state_dir.join(format!(
        "{RETIRED_LOCK_PREFIX}{}.{}.{}",
        current_info.dev(),
        current_info.ino(),
        replacement_token
    ));
    std::fs::hard_link(&canonical, &retired)?;
    if !path_names_file(&canonical, &current) {
        let _ = std::fs::remove_file(&retired);
        return Ok(LockState::Busy {
            holder: Some(holder),
        });
    }

    let staged = state_dir.join(format!(".{LOCK_NAME}.takeover.{replacement_token}"));
    let fresh = open_lock_at(&staged)?;
    if let Err(error) = fs2::FileExt::try_lock_exclusive(&fresh) {
        let _ = std::fs::remove_file(&staged);
        let _ = std::fs::remove_file(&retired);
        return Err(error.into());
    }
    if let Err(error) = std::fs::rename(&staged, &canonical) {
        let _ = std::fs::remove_file(&staged);
        let _ = std::fs::remove_file(&retired);
        return Err(error.into());
    }
    let (token, records) = match write_lock_holder(state_dir, &fresh, pass_seconds, writer) {
        Ok(holder) => holder,
        Err(error) => {
            // Put the predecessor inode back under the canonical name. Its
            // holder record still describes it, so the next pass can retry.
            let _ = std::fs::rename(&retired, &canonical);
            return Err(error);
        }
    };
    Ok(LockState::TakenOver {
        lock: ExclusiveLock {
            file: fresh,
            holder_records: records,
            holder_token: Some(token),
        },
        from_pid: holder.pid,
        overdue_seconds,
    })
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

/// One writer's own last attempt, or `None` when it has never recorded one.
///
/// `None` means run: a writer that has never stamped the file has no interval
/// to be inside. That is also the upgrade path - the first pass by each writer
/// after this change runs once immediately, because the old file carries only
/// the shared `last_attempt_at`.
fn writer_last_attempt(state: &Value, writer: &str) -> Option<f64> {
    state
        .get(WRITER_ATTEMPTS)
        .and_then(Value::as_object)
        .and_then(|stamps| stamps.get(writer))
        .and_then(Value::as_f64)
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
    // Every writer's stamp is carried forward and only this one is updated.
    // The interval gate reads the stamp belonging to the writer about to run,
    // so dropping the others here would restore the starvation this exists to
    // end: one process's pass would clear the record another gates on.
    let previous = read_state(state_dir).unwrap_or_else(|_| Value::Object(Map::new()));
    let mut by_writer = previous
        .get(WRITER_ATTEMPTS)
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    if let Some(writer) = report.get("writer").and_then(Value::as_str) {
        by_writer.insert(writer.to_string(), serde_json::json!(attempted_at));
    }
    // `last_attempt_at` keeps its meaning - the last attempt by ANYONE, which
    // is what `host disk` reports and what `next_pass_at` is computed from -
    // and never moves backwards. An `interval_noop` anchors on its own older
    // stamp, and writing that verbatim would rewind a newer pass by another
    // writer.
    let last_attempt_at = previous
        .get("last_attempt_at")
        .and_then(Value::as_f64)
        .map_or(attempted_at, |recorded| recorded.max(attempted_at));
    // When the last pass was prevented rather than run, and never moving
    // backwards either.
    //
    // A janitor that cannot take the run lock because a workload holds it in
    // shared mode has been PREVENTED. That is a modelled, healthy answer —
    // `acquire_workload_lock` takes the lock for the job's whole duration on
    // purpose — but until this stamp existed nothing recorded it, so
    // `cleanup_success_age_seconds` downstream could not tell a prevented
    // janitor from a silent one and inferred a stall from the absence. On
    // 2026-09-03 charless-mac-mini ran one job for 42 minutes, roughly 40
    // in-process passes hit `lock_busy` at the ten-second agent tick, none of
    // them left a trace, and `host gates` turned `claiming` off on a host with
    // 17.3 GiB free, a 15 GiB watermark and `disk_pressure_unresolved: false`.
    //
    // Only the time is recorded here, not the holder: a `flock` owner cannot be
    // named from the process that failed to take it, and the one thing in this
    // product that can name it -- `host disk`'s `cleanup_lock.holders` --
    // already does. What the arithmetic needs is prevented-since-a-known-time,
    // and that is what this is.
    let prevented_now = report.get("outcome").and_then(Value::as_str) == Some("lock_busy");
    let last_prevented_at = previous
        .get("last_prevented_at")
        .and_then(Value::as_f64)
        .map_or_else(
            || prevented_now.then_some(attempted_at),
            |recorded| {
                Some(if prevented_now {
                    recorded.max(attempted_at)
                } else {
                    recorded
                })
            },
        );
    // A prevented pass returns before the lock is held, so it never reached the
    // carry-forward in `run_with_lock` and its report says `last_success_at:
    // null`. Writing that verbatim would erase the one timestamp the stall
    // arithmetic is measured from — recording the prevented pass would then be
    // worse than dropping it. The last success belongs to the host, not to a
    // pass, so it is carried here for every writer and every outcome.
    let mut report = report.clone();
    if report.get("last_success_at").is_none_or(Value::is_null) {
        if let Some(recorded) = previous
            .get("report")
            .and_then(|previous| previous.get("last_success_at"))
            .filter(|stamp| !stamp.is_null())
        {
            if let Some(object) = report.as_object_mut() {
                object.insert("last_success_at".to_string(), recorded.clone());
            }
        }
    }
    let mut state = Map::new();
    state.insert("version".to_string(), serde_json::json!(STATE_VERSION));
    state.insert(
        "last_attempt_at".to_string(),
        serde_json::json!(last_attempt_at),
    );
    if let Some(stamp) = last_prevented_at {
        state.insert("last_prevented_at".to_string(), serde_json::json!(stamp));
    }
    state.insert(WRITER_ATTEMPTS.to_string(), Value::Object(by_writer));
    state.insert("report".to_string(), report);
    let payload = canonical_json(&Value::Object(state));
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
const PUBLIC_OUTCOMES: [&str; 13] = [
    "never_run",
    "invalid_or_unavailable_policy",
    "lock_busy",
    "interval_noop",
    "healthy_noop",
    "report_only",
    "lock_recovery_report_only",
    "blocked_running_jobs",
    "reclaimed_target",
    "reclaimed_progress",
    "cap_reached",
    "partial_error",
    "no_eligible_items",
];

/// Python `_PUBLIC_SKIP_REASONS`. NOTE: the weles-internal reasons
/// `active_run`, `escapes_root`, and `item_cap` are deliberately absent
/// (they never leave the host), exactly as in the Python source.
const PUBLIC_SKIP_REASONS: [&str; 16] = [
    "active_jobs",
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
    // A pass that did not reach its cleaners carries no table
    // ([`CleanupReport::scanned`]), and the public form has to keep saying so:
    // filling the six sections with zeros here would rebuild, one layer out,
    // exactly the "did not run" that reads as "nothing needed doing".
    let public_cleaners = match cleaners {
        None => Value::Null,
        Some(_) => serde_json::json!({
            "huggingface_cache": public_cleaner(cleaners.and_then(|c| c.get("huggingface_cache"))),
            "weles_recordings": public_cleaner(cleaners.and_then(|c| c.get("weles_recordings"))),
            "build_caches": public_cleaner(cleaners.and_then(|c| c.get("build_caches"))),
            chromium_clones::CLEANER: public_cleaner(
                cleaners.and_then(|c| c.get(chromium_clones::CLEANER)),
            ),
            queue_workdirs::CLEANER: public_cleaner(
                cleaners.and_then(|c| c.get(queue_workdirs::CLEANER)),
            ),
            backup_twins::CLEANER: public_cleaner(
                cleaners.and_then(|c| c.get(backup_twins::CLEANER)),
            ),
            release_store::CLEANER: public_cleaner(
                cleaners.and_then(|c| c.get(release_store::CLEANER)),
            ),
        }),
    };
    // The declared cleaners the pass never reached, kept in the public form
    // because the reader that needs it is `stado host disk` on another
    // machine. Filtered to the six known cleaner names: this crosses a host
    // boundary into an operator's terminal, and every other field here is
    // bounded for the same reason.
    let public_unscanned: Vec<Value> = get("unscanned_cleaners")
        .and_then(Value::as_array)
        .map(|names| {
            names
                .iter()
                .filter_map(Value::as_str)
                .filter(|name| {
                    matches!(
                        *name,
                        "huggingface_cache" | "weles_recordings" | "build_caches"
                    ) || *name == chromium_clones::CLEANER
                        || *name == queue_workdirs::CLEANER
                        || *name == backup_twins::CLEANER
                })
                .map(Value::from)
                .collect()
        })
        .unwrap_or_default();
    serde_json::json!({
        "version": STATE_VERSION,
        "mode": mode,
        "check_interval_seconds": public_nonnegative(get("check_interval_seconds")),
        "started_at": public_timestamp(get("started_at")),
        "duration_ms": public_nonnegative(get("duration_ms")).unwrap_or(0),
        "store_wait_ms": public_nonnegative(get("store_wait_ms")).unwrap_or(0),
        "outcome": outcome,
        "free_bytes_before": public_nonnegative(get("free_bytes_before")),
        "free_bytes_after": public_nonnegative(get("free_bytes_after")),
        "low_bytes": public_nonnegative(get("low_bytes")),
        "target_bytes": public_nonnegative(get("target_bytes")),
        "pressure_active": get("pressure_active").and_then(Value::as_bool),
        "cleaners": public_cleaners,
        "unscanned_cleaners": public_unscanned,
        "caps": {
            "bytes": cap("bytes"),
            "items": cap("items"),
            "scan": cap("scan"),
            "deadline": cap("deadline"),
        },
        "lock_busy": lock_busy || get("lock_busy") == Some(&Value::Bool(true)),
        "active_job_count": public_nonnegative(get("active_job_count")).unwrap_or(0),
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

const SERVICE_LOG_MAX_BYTES: u64 = 64 * 1024 * 1024;
const SERVICE_LOG_KEEP_BYTES: u64 = 4 * 1024 * 1024;
const SERVICE_LOG_SCAN_LIMIT: usize = 512;

/// Bound owner-written service logs even while the host has ample free space.
///
/// launchd appends forever to `StandardOutPath` and `StandardErrorPath`; disk
/// pressure is too late to enforce a per-file bound. Keep the newest 4 MiB in
/// place so an already-open `O_APPEND` descriptor continues writing the same
/// inode. Symlinks, hard links, foreign owners, and non-log files are refused.
fn rotate_service_logs(home: &Path, log_fn: &mut dyn FnMut(&str)) {
    let root = home.join(".stado").join("logs");
    let entries = match std::fs::read_dir(&root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return,
        Err(error) => {
            log_fn(&format!("service log scan failed: {error}"));
            return;
        }
    };
    let owner = unsafe { nix::libc::geteuid() };
    for entry in entries.take(SERVICE_LOG_SCAN_LIMIT) {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                log_fn(&format!("service log entry unreadable: {error}"));
                continue;
            }
        };
        let path = entry.path();
        if !matches!(
            path.extension().and_then(|value| value.to_str()),
            Some("log" | "out" | "err")
        ) {
            continue;
        }
        let metadata = match entry.metadata() {
            Ok(metadata)
                if ifmt(metadata.mode()) == IFREG
                    && metadata.uid() == owner
                    && metadata.nlink() == 1
                    && metadata.len() > SERVICE_LOG_MAX_BYTES =>
            {
                metadata
            }
            Ok(_) => continue,
            Err(error) => {
                log_fn(&format!("service log metadata unreadable: {error}"));
                continue;
            }
        };
        let result = (|| -> io::Result<u64> {
            let mut file = OpenOptions::new()
                .read(true)
                .write(true)
                .custom_flags(nix::libc::O_NOFOLLOW)
                .open(&path)?;
            let opened = file.metadata()?;
            if ifmt(opened.mode()) != IFREG
                || opened.uid() != owner
                || opened.nlink() != 1
                || opened.len() <= SERVICE_LOG_MAX_BYTES
            {
                return Ok(opened.len());
            }
            let start = opened.len().saturating_sub(SERVICE_LOG_KEEP_BYTES);
            file.seek(SeekFrom::Start(start))?;
            let mut tail = Vec::with_capacity((opened.len() - start) as usize);
            (&mut file)
                .take(SERVICE_LOG_KEEP_BYTES)
                .read_to_end(&mut tail)?;
            file.seek(SeekFrom::Start(0))?;
            file.write_all(&tail)?;
            file.set_len(tail.len() as u64)?;
            file.sync_data()?;
            Ok(tail.len() as u64)
        })();
        match result {
            Ok(after) if after < metadata.len() => log_fn(&format!(
                "service log rotated file={} bytes_before={} bytes_after={after}",
                entry.file_name().to_string_lossy(),
                metadata.len()
            )),
            Ok(_) => {}
            Err(error) => log_fn(&format!(
                "service log rotation failed file={}: {error}",
                entry.file_name().to_string_lossy()
            )),
        }
    }
}

// ---------------------------------------------------------------------------
// canonical policy resolution
// ---------------------------------------------------------------------------

/// Fetch the canonical registry through this process's configured primary;
/// destructive checks never use fallback/cache. Generation-pinned via the
/// store's versioned read (reload + pinned download, with the same 412-retry
/// the Python SDK path relies on).
///
/// The registry remains fail-closed: malformed, incomplete, or unreadable
/// policy never authorizes deletion. A client configured with the Stado object
/// adapter must read that authority. Its separately configured backup is not a
/// substitute: it may be intentionally distinct, stale, or unable to represent
/// the primary namespace at all.
///
/// DEVIATION from Python, matching `targets::download_registry_blob`: the
/// object is resolved by [`targets::RegistryStore`] instead of a hardcoded GCS
/// bucket.
async fn fetch_canonical_registry() -> Result<Value, JanitorError> {
    let store = targets::RegistryStore::open_primary_reads().await?;
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
/// No package registry fallback is permitted. Any fetch, schema, identity, or
/// typing failure is propagated to the caller, which must fail closed.
///
/// A refusal by [`targets::validate_registry`] is NOT a malformed document
/// and is not journalled as one. The parse already succeeded — `data` is a
/// `Value` — so what this rejects is a well-formed registry declaring
/// something this build does not implement, and it carries
/// [`JanitorError::unsupported`]: the entry reads
/// `policy:NotImplementedError` instead of the `policy:ValueError` a corrupt
/// document produces in [`fetch_canonical_registry`].
///
/// The fourth element is true when the host declared no policy and
/// [`DiskCleanupPolicy::reporting_default`] is in force, so a report can say
/// which of the two an operator is looking at. Python
/// `resolve_canonical_policy`.
pub fn resolve_canonical_policy(
    data: &Value,
    hostname: &str,
) -> Result<(ComputeTarget, DiskCleanupPolicy, String, bool), JanitorError> {
    targets::validate_registry(data).map_err(|exc| JanitorError::unsupported(&exc.to_string()))?;
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
    if raw.get("kind").and_then(Value::as_str) != Some("local") {
        return Err(JanitorError::lookup(
            "matched target is not a local host, so it has no local cleanup policy",
        ));
    }
    let target: ComputeTarget = serde_json::from_value(Value::Object(raw.clone()))
        .map_err(|_| JanitorError::lookup("cleanup policy could not be parsed"))?;
    // A declared policy wins. A local host that declares none is measured
    // against `DiskCleanupPolicy::reporting_default` rather than refused:
    // returning an error here meant an undeclared host was never scanned and
    // its report carried a `policy` error instead of a free-space number, so
    // the one host that builds every release filled to 1.8 GiB free with
    // nobody watching. Silence in the registry is "nobody has said", not
    // "nothing to do".
    //
    // The digest still comes from whatever policy is in force, so the state
    // file's fencing is unchanged; `defaulted` is what tells the report, and
    // an operator, that no declaration exists.
    let (policy, canonical, defaulted) = match target.disk_cleanup.clone() {
        Some(policy) => (policy, canonical_json(&raw["disk_cleanup"]), false),
        None => {
            let policy = crate::targets::DiskCleanupPolicy::reporting_default();
            let rendered = serde_json::to_value(&policy)
                .map_err(|_| JanitorError::lookup("default cleanup policy could not be built"))?;
            (policy, canonical_json(&rendered), true)
        }
    };
    let digest = format!("{:x}", Sha256::digest(canonical.as_bytes()));
    Ok((target, policy, digest, defaulted))
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

/// Every job id named by `queue` or `running`, without downloading job
/// documents. Transition sentinels stay on this conservative set.
async fn listed_live_job_ids(store: &crate::queue::JobStorage) -> Option<BTreeSet<String>> {
    let mut ids = BTreeSet::new();
    for state in ["queue", "running"] {
        ids.extend(store.list_job_ids(state).await.ok()?);
    }
    Some(ids)
}

/// Build the conservative listing-only keep set inside `budget`.
///
/// Public as the existing seam that proves no queue document is downloaded.
pub async fn live_job_ids_within(
    store: &crate::queue::JobStorage,
    budget: Duration,
) -> Option<Vec<String>> {
    tokio::time::timeout(budget, listed_live_job_ids(store))
        .await
        .ok()
        .flatten()
        .map(BTreeSet::into_iter)
        .map(Iterator::collect)
}

/// Refine the listing-only keep set for the bounded workdir population.
///
/// Only ids that exist on disk and whose names occur in both a live and a
/// terminal prefix cost authoritative document reads. The typed storage query
/// accepts only a retired transition with its matching terminal destination
/// and no live or in-flight source. Any failed list/read or timeout makes the
/// whole keep-list unavailable, so the workdir cleaner deletes nothing.
async fn live_job_ids_for_candidates_within(
    store: &crate::queue::JobStorage,
    candidates: &BTreeSet<String>,
    budget: Duration,
) -> Option<Vec<String>> {
    let read = async {
        let mut live = listed_live_job_ids(store).await?;
        let mut terminal_names = BTreeSet::new();
        for prefix in crate::queue::runs::TERMINAL_PREFIXES {
            terminal_names.extend(store.list_job_ids(prefix).await.ok()?);
        }
        for job_id in candidates {
            if !live.contains(job_id) || !terminal_names.contains(job_id) {
                continue;
            }
            match store.workdir_job_state(job_id).await.ok()? {
                crate::queue::storage::WorkdirJobState::Terminal => {
                    live.remove(job_id);
                }
                crate::queue::storage::WorkdirJobState::Live
                | crate::queue::storage::WorkdirJobState::Unknown => {}
            }
        }
        Some(live.into_iter().collect())
    };
    tokio::time::timeout(budget, read).await.ok().flatten()
}

/// Build the refined keep-list against this process's configured authoritative
/// primary. Construction and layout validation are part of the same budget as
/// every listing and versioned read.
async fn fetch_live_job_ids(
    candidates: &BTreeSet<String>,
    budget: Duration,
) -> Option<Vec<String>> {
    let read = async {
        let store = crate::queue::JobStorage::for_primary_reads().await.ok()?;
        live_job_ids_for_candidates_within(&store, candidates, budget).await
    };
    tokio::time::timeout(budget, read).await.ok().flatten()
}

/// The post-lock half of `run_cleanup_once` (policy resolution through
/// outcome selection). Split out so tests can inject the canonical registry
/// document and a fabricated home without touching GCS or the real `$HOME`.
/// `_lock` holds the exclusive run lock through candidate enumeration,
/// authoritative state reads, and deletion.
#[allow(clippy::too_many_arguments)]
async fn run_with_lock(
    home: &Path,
    state_dir: &Path,
    _lock: ExclusiveLock,
    registry: Result<Value, JanitorError>,
    mut report: CleanupReport,
    started: Instant,
    attempted_at: f64,
    force: bool,
    // Plan only: pin an `enforce` policy down to the janitor's own `report`
    // mode and persist nothing. See `preview_cleanup_once`.
    preview: bool,
    // A predecessor lock inode is still live, or was replaced during this
    // pass. Scan and persist diagnostics, but never delete until every
    // predecessor kernel lock has actually been released.
    lock_recovery: bool,
    log_fn: &mut dyn FnMut(&str),
) -> Value {
    // A preview leaves no trace. The state file is the janitor's record of
    // REAL passes: writing it would advance this writer's attempt stamp, so an
    // operator asking what a cleanup WOULD delete would have silently
    // delayed the cleanup that does.
    let persist = if preview { None } else { Some(state_dir) };
    let (target, mut policy, digest, policy_defaulted) =
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
    // preview and lock recovery are this pass with that one word changed,
    // not second implementations of the policy.
    //
    // `off` and `report` policies are left exactly as the registry states.
    if (preview || lock_recovery) && policy.mode == "enforce" {
        policy.mode = "report".to_string();
    }
    report.target_name = Some(target.name);
    report.policy_digest = Some(digest.clone());
    report.mode = Some(policy.mode.clone());
    report.check_interval_seconds = Some(policy.check_interval_seconds);
    report.low_bytes = Some(policy.low_free_gb * GIB);
    report.target_bytes = Some(policy.target_free_gb * GIB);
    report.policy_defaulted = policy_defaulted;

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
    // Where the previous pass's build-cache walk stopped. Without this the
    // walk restarts at its root every pass and a tree larger than one pass's
    // budget is never crossed: on 2026-09-01 `lukasz-macbook` held 879,559
    // directories under the declared root against a `max_scan_items` ceiling
    // of 100,000, so the same first eleven percent was scanned hourly and the
    // caches in the rest were unreachable by construction.
    report.builds_resume_from = previous_report
        .as_ref()
        .and_then(|r| r.get("build_caches_resume_from"))
        .and_then(|v| v.as_str().map(str::to_string));
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
    // THIS writer's last attempt, not the file's.
    //
    // This read used to be `previous["last_attempt_at"]` - the last attempt by
    // anyone - so any writer's stamp gated every writer. On 2026-08-31
    // charless-mac-mini had two janitors: the queue agent's in-process pass and
    // a standalone `disk-cleanup` unit on its own timer. With the thresholds
    // raised to 40/42 GiB against 31.2 GiB free, the agent reported
    // `disk_pressure_active: true`, `errors: []`, policy resolved, and all six
    // cleaners `scanned 0` - because the other process had stamped the file
    // within the interval. Pressure active, policy resolved, nothing scanned.
    //
    // The gate returns before the first scanner AND before `run_with_lock`
    // reaches the lock, so the lock cannot mediate it: the lock makes two
    // janitors take turns deleting, while this made the working one never try.
    // Both are real and only this one silences a pass.
    //
    // Removing a redundant unit does not fix this. `stado disk-cleanup --once`
    // is a supported operator command that writes the same file, so one manual
    // run would otherwise silence the agent's janitor for a full interval on
    // any host.
    let last_attempt = writer_last_attempt(&previous, report.writer);
    // The interval paces a healthy host; it must not pace a host that is
    // already under its own declared low watermark. Measuring free space
    // above this gate is what makes that possible, and it is also what makes
    // an `interval_noop` report readable: the gate used to return before the
    // first measurement, so the report said `pressure_active: null` while the
    // same agent's log line said `disk-pressure-active`. On 2026-09-02
    // `lukasz-macbook` sat at 9.3 GB free against a 100 GB watermark and its
    // janitor answered `interval_noop` every pass, hourly, while the release
    // pipeline kept writing. `cap_reached` plus `continuing_reclaim` already
    // model a reclaim that must be resumed - and this gate was what refused
    // to resume it. Concurrency is mediated by the lock below, never by this
    // stamp: the lock makes two janitors take turns, the stamp made the
    // working one never try.
    if !force
        && report.pressure_active != Some(true)
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
    if !preview && policy.mode == "enforce" {
        rotate_service_logs(home, log_fn);
    }
    if policy.mode == "off" || report.pressure_active != Some(true) {
        report.outcome = "healthy_noop".to_string();
        report.last_success_at = Some(utc_now());
        return finish(report, started, Some(home), persist, attempted_at, log_fn);
    }

    // The host's declared pass budget, or this module's own 30 seconds when it
    // declares none. This is the limit that actually decides how much of a
    // large tree one pass sees: on `lukasz-macbook` `max_scan_items` never
    // bound and the deadline did, every pass.
    let pass_seconds = policy
        .max_pass_seconds
        .filter(|seconds| *seconds > 0)
        .map_or(DEADLINE_SECONDS, |seconds| seconds as f64);
    let deadline = Instant::now() + std::time::Duration::from_secs_f64(pass_seconds);
    // How much of the pass's remaining scan budget one cleaner may spend while
    // declared cleaners behind it have not run yet.
    //
    // Every cleaner used to receive `max_scan_items` minus what the ones
    // before it had spent, which reads as fair and is not: the cleaners run in
    // a fixed order, and one whose root is large enough to exhaust the cap
    // takes the whole pass, every pass, forever. Measured on
    // charless-mac-mini on 2026-08-31 with `max_scan_items: 10000` and six
    // declared cleaners: `weles_recordings` scanned 15, `build_caches` scanned
    // 9,985 and found NOTHING eligible, and `chromium_clones`,
    // `queue_workdirs` and `backup_twins` each received a budget of zero and
    // scanned nothing — pass after pass, under real disk pressure, with 18 GiB
    // of proven duplicates sitting in the replica that `backup_twins` exists
    // to reclaim. The outcome was `cap_reached`, which is true and reads like
    // work being done.
    //
    // An equal share of what is left, with everything unspent rolling forward
    // to the cleaners behind: a cleaner that scans less than its share leaves
    // more for the rest, and the last declared cleaner is handed whatever
    // remains. No cleaner is ever handed zero while it is declared, which is
    // the property that was missing.
    const CLEANER_ORDER: [&str; 7] = [
        "huggingface_cache",
        "weles_recordings",
        "build_caches",
        chromium_clones::CLEANER,
        queue_workdirs::CLEANER,
        backup_twins::CLEANER,
        release_store::CLEANER,
    ];
    let declared_after = |current: &str| -> i64 {
        CLEANER_ORDER
            .iter()
            .skip_while(|name| **name != current)
            .skip(1)
            .filter(|name| policy.cleaners.contains_key(**name))
            .count() as i64
    };
    let share = |remaining: i64, behind: i64| -> i64 {
        if behind <= 0 {
            remaining
        } else {
            (remaining / (behind + 1)).max(1).min(remaining)
        }
    };
    // Item shares alone do not make the fixed order fair: a cleaner can spend
    // the whole wall-clock allowance while staying inside its item share. Give
    // each declared cleaner an equal slice of the time that remains at the
    // instant it starts. Any unused slice stays inside the single global
    // deadline and is therefore rolled into the next cleaner's calculation.
    let time_share = |current: &str| -> Instant {
        let now = Instant::now();
        let remaining = deadline.saturating_duration_since(now);
        let slots = declared_after(current).saturating_add(1) as u32;
        now + remaining / slots
    };
    // Past every early return: from here the cleaner table is a measurement
    // this pass actually made, so the report may carry one.
    report.scanned = true;
    // Errors escaping _run_hf (a vanished cache root mid-pass, a failed
    // free-space probe) hit Python's outer `except BaseException`:
    // `runtime` error + the default outcome, state still written.
    if let Err(exc) = hf::run_hf(
        home,
        &policy,
        report.active_job_count,
        attempted_at,
        share(
            policy.max_scan_items,
            declared_after("huggingface_cache"),
        ),
        time_share("huggingface_cache"),
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
    weles::scan_weles(
        home,
        &policy,
        attempted_at,
        share(
            remaining_scan,
            declared_after("weles_recordings"),
        ),
        time_share("weles_recordings"),
        &mut report,
    );
    let remaining_after_weles =
        (policy.max_scan_items - report.hf.scanned_items - report.weles.scanned_items).max(0);
    if remaining_after_weles == 0 && policy.cleaners.contains_key("build_caches") {
        report.caps.scan = true;
    }
    // The build-cache scan is the only one whose root can be the whole of
    // `$HOME`: it walks with its item and time shares of what the fixed-layout
    // cleaners left. It is also the only one that cannot finish in one pass on
    // a large tree, so it resumes from where the last pass stopped instead of
    // restarting.
    build_caches::scan_build_caches(
        home,
        &policy,
        attempted_at,
        share(
            remaining_after_weles,
            declared_after("build_caches"),
        ),
        time_share("build_caches"),
        report.builds_resume_from.clone(),
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
    // The only cleaner whose root is outside this account's home: macOS puts
    // the clones in the per-user temporary container. Its unused item and time
    // shares roll forward to the lifecycle cleaners behind it.
    chromium_clones::scan_chromium_clones(
        home,
        &policy,
        attempted_at,
        share(
            remaining_after_builds,
            declared_after(chromium_clones::CLEANER),
        ),
        time_share(chromium_clones::CLEANER),
        &mut report,
    );
    let remaining_after_clones = (policy.max_scan_items
        - report.hf.scanned_items
        - report.weles.scanned_items
        - report.builds.scanned_items
        - report.clones.scanned_items)
        .max(0);
    if remaining_after_clones == 0 && policy.cleaners.contains_key(queue_workdirs::CLEANER) {
        report.caps.scan = true;
    }
    // The queue's own per-job trees, after rebuildable caches.
    // Candidate names are captured under this same janitor lock. The queue
    // authority then downloads bodies only for candidates whose names overlap
    // live and terminal prefixes; everything unreadable remains on the keep
    // set, and any store failure disables this cleaner for the whole pass.
    let workdir_budget = share(
        remaining_after_clones,
        declared_after(queue_workdirs::CLEANER),
    );
    let workdir_deadline = time_share(queue_workdirs::CLEANER);
    let live_jobs = if workdir_budget > 0 && policy.cleaners.contains_key(queue_workdirs::CLEANER) {
        match queue_workdirs::candidate_job_ids(home, workdir_budget, workdir_deadline) {
            Ok(candidates) => {
                let budget = workdir_deadline
                    .saturating_duration_since(Instant::now())
                    .min(KEEP_LIST_BUDGET);
                let wait = Instant::now();
                let ids = fetch_live_job_ids(&candidates, budget).await;
                report.store_wait_ms = report
                    .store_wait_ms
                    .saturating_add(wait.elapsed().as_millis().min(i64::MAX as u128) as i64);
                ids
            }
            Err(error) => {
                report.add_error(queue_workdirs::CLEANER, &error);
                None
            }
        }
    } else {
        Some(Vec::new())
    };
    queue_workdirs::scan_queue_workdirs(
        home,
        &policy,
        attempted_at,
        workdir_budget,
        workdir_deadline,
        live_jobs.as_deref(),
        &mut report,
    );
    let remaining_after_workdirs = (policy.max_scan_items
        - report.hf.scanned_items
        - report.weles.scanned_items
        - report.builds.scanned_items
        - report.clones.scanned_items
        - report.workdirs.scanned_items)
        .max(0);
    if remaining_after_workdirs == 0 && policy.cleaners.contains_key(backup_twins::CLEANER) {
        report.caps.scan = true;
    }
    // The disaster-recovery replica's proven duplicates. It is the only
    // cleaner here that has to READ the bytes it deletes: every object it
    // removes is hashed against the primary in this same pass. Release-store
    // cleanup remains behind it and receives its own item/time share.
    let twins_budget = share(
        remaining_after_workdirs,
        declared_after(backup_twins::CLEANER),
    );
    backup_twins::scan_backup_twins(
        home,
        &policy,
        crate::config::wc_stado_storage_namespace(),
        twins_budget,
        time_share(backup_twins::CLEANER),
        &mut report,
    );
    let remaining_after_twins = (policy.max_scan_items
        - report.hf.scanned_items
        - report.weles.scanned_items
        - report.builds.scanned_items
        - report.clones.scanned_items
        - report.workdirs.scanned_items
        - report.backup_twins.scanned_items)
        .max(0);
    if remaining_after_twins == 0 && policy.cleaners.contains_key(release_store::CLEANER) {
        report.caps.scan = true;
    }
    // Immutable release versions nothing on this host still names, scanned
    // last: it deletes whole version directories, so it takes the smallest
    // share and only after every cleaner that reclaims scratch has had its
    // turn — a release is the one class here that costs a rebuild to get
    // back.
    release_store::scan_release_store(home, &policy, remaining_after_twins, deadline, &mut report);
    let total_scanned = report.hf.scanned_items
        + report.weles.scanned_items
        + report.builds.scanned_items
        + report.clones.scanned_items
        + report.workdirs.scanned_items
        + report.backup_twins.scanned_items
        + report.release_store.scanned_items;
    if total_scanned >= policy.max_scan_items {
        report.caps.scan = true;
    }
    // Which declared cleaners never got a turn. Every counter needed for this
    // was already in hand here and nothing said it: a cleaner whose share ran
    // out publishes the same three zeros as one that looked and found nothing,
    // and `cap_reached` names the budget rather than the cleaner it stopped.
    //
    // Keyed on the two skips a budget produces — `scan_cap` and
    // `scan_deadline` — and never on a zero count alone: a cleaner whose root
    // does not exist on this host also scans nothing, reports `root_absent`,
    // and is not waiting for a turn. Calling that one unscanned would be this
    // field committing the error it exists to report. The order is the run
    // order, so the answer reads as "the pass ended before these".
    let budget_stopped = |cleaner: &CleanerReport| {
        cleaner.scanned_items == 0
            && (cleaner.skipped.contains_key("scan_cap")
                || cleaner.skipped.contains_key("scan_deadline"))
    };
    report.unscanned_cleaners = [
        ("huggingface_cache", &report.hf),
        ("weles_recordings", &report.weles),
        ("build_caches", &report.builds),
        (chromium_clones::CLEANER, &report.clones),
        (queue_workdirs::CLEANER, &report.workdirs),
        (backup_twins::CLEANER, &report.backup_twins),
        (release_store::CLEANER, &report.release_store),
    ]
    .into_iter()
    .filter(|(name, cleaner)| policy.cleaners.contains_key(*name) && budget_stopped(cleaner))
    .map(|(name, _)| name.to_string())
    .collect();
    report.unknown_cleaners = policy
        .cleaners
        .keys()
        .filter(|name| !CLEANER_ORDER.contains(&name.as_str()))
        .cloned()
        .collect();

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
    let deleted = report.hf.deleted_items
        + report.builds.deleted_items
        + report.clones.deleted_items
        + report.backup_twins.deleted_items;
    // An incomplete scan is named before anything else a complete pass could
    // have concluded. `cap_reached` is the report's existing word for "a
    // budget stopped me", and it has to win here: a pass that spent its scan
    // cap without reaching a candidate used to publish `report_only` (or, in
    // `enforce`, `no_eligible_items` once the caps happened to be clear),
    // and both of those read as a finished look at the disk. On
    // `lukasz-macbook` that was the whole difference between "there is
    // nothing to delete" and "I got 7,297 directories into one repository's
    // `node_modules` and ran out of time", with 174 tagged build caches and
    // a 62 GiB `target/` unexamined behind it.
    if lock_recovery {
        report.outcome = "lock_recovery_report_only".to_string();
    } else if report.caps.any() && deleted == 0 && after < policy.target_free_gb * GIB {
        report.outcome = "cap_reached".to_string();
    } else if policy.mode != "enforce" {
        report.outcome = "report_only".to_string();
    } else if report.active_job_count > 0 && policy.cleaners.contains_key("huggingface_cache") {
        report.outcome = "blocked_running_jobs".to_string();
    } else if after >= policy.target_free_gb * GIB {
        report.outcome = "reclaimed_target".to_string();
    } else if report.caps.any() {
        report.outcome = "cap_reached".to_string();
    } else if !report.errors.is_empty() {
        report.outcome = "partial_error".to_string();
    } else if deleted == 0 {
        report.outcome = "no_eligible_items".to_string();
    } else {
        report.outcome = "reclaimed_progress".to_string();
    }
    if report.errors.is_empty() {
        report.last_success_at = Some(utc_now());
    }
    finish(report, started, Some(home), persist, attempted_at, log_fn)
}

/// Which process made a pass.
///
/// The state file has more than one writer on an always-on host: the queue
/// agent runs a pass every tick, and a `disk-cleanup --watch` unit runs one on
/// its own timer. [`crate::deploy::host_gates`] already documents what that
/// costs — it read a `low watermark 20 GiB, target 18 GiB` from a stale policy
/// alternating with the canonical 15/18 between one reading and the next — and
/// solved it for watermarks by preferring the registry declaration.
///
/// An `outcome` cannot be solved that way, because it is an event and not a
/// declaration. On 2026-08-31 the agent's pass at 14:55:24Z reported
/// `interval_noop` with no errors and all six cleaners scanned, and 46 seconds
/// later `stado host disk` read `invalid_or_unavailable_policy` from the same
/// path: two processes, opposite verdicts, and the operator's answer decided by
/// which wrote last. A long-running writer holding a superseded configuration —
/// or an older binary that rejects a cleaner the registry now declares, which
/// makes it reject the whole document and resolve no policy at all — loses
/// nothing by overwriting a healthy report.
///
/// So every pass now says who made it and with which version. That does not
/// arbitrate between writers, and deliberately so: the file is the last pass by
/// whoever made it, which is a true thing to be. What changes is that a reader
/// can say so instead of presenting one process's verdict as the host's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CleanupWriter {
    /// The queue agent's per-tick pass.
    AgentTick,
    /// `stado disk-cleanup`, whether `--once` or under a `--watch` unit.
    Cli,
}

impl CleanupWriter {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::AgentTick => "agent-tick",
            Self::Cli => "disk-cleanup-cli",
        }
    }
}

/// Resolve canonical policy and execute at most one bounded cleanup pass.
/// Python `run_cleanup_once`. Never fails: every failure mode lands in
/// the returned report (the agent mirrors Python's outcome handling).
pub async fn run_cleanup_once(
    active_job_count: i64,
    force: bool,
    writer: CleanupWriter,
    log_fn: &mut dyn FnMut(&str),
) -> Value {
    cleanup_once(active_job_count, force, false, writer, log_fn).await
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
/// and the preview carries zero running jobs because it is not the worker.
///
/// `stado disk-cleanup --dry-run` runs this locally;
/// `stado host cleanup TARGET --dry-run`
/// ([`crate::deploy::host_cleanup`]) runs it over ssh on the host being
/// previewed, which is the only place the host's filesystem exists.
pub async fn preview_cleanup_once(log_fn: &mut dyn FnMut(&str)) -> Value {
    // A preview persists nothing, so its writer identity never reaches the
    // file; it is recorded anyway so the returned report is self-describing.
    cleanup_once(i64::default(), true, true, CleanupWriter::Cli, log_fn).await
}

/// The shared body of [`run_cleanup_once`] and [`preview_cleanup_once`].
async fn cleanup_once(
    active_job_count: i64,
    force: bool,
    preview: bool,
    writer: CleanupWriter,
    log_fn: &mut dyn FnMut(&str),
) -> Value {
    let started = Instant::now();
    let attempted_at = epoch_now();
    let hostname = crate::providers::vast::system_hostname();
    let mut report = CleanupReport::base(active_job_count, &hostname);
    report.writer = writer.as_str();
    report.writer_version = crate::build_identity::BUILD_IDENTITY;

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
    // Resolve the canonical policy before taking the exclusive janitor lock.
    // It has no filesystem side effects, and an unavailable authority fails
    // closed before blocking another cleanup process.
    //
    // The workdir keep-list is different: its candidate names must be captured
    // under the same lock that protects deletion. `run_with_lock` performs that
    // candidate-bounded authority read immediately before the workdir cleaner,
    // inside both the store-read budget and the pass deadline.
    let store_wait = Instant::now();
    let input_budget = Duration::from_secs(constants::AGENT_STORE_READ_TIMEOUT_S);
    let registry = match tokio::time::timeout(input_budget, fetch_canonical_registry()).await {
        Ok(result) => result,
        Err(_) => Err(JanitorError::timeout(&format!(
            "canonical registry did not answer within {}s",
            constants::AGENT_STORE_READ_TIMEOUT_S
        ))),
    };
    report.store_wait_ms = store_wait.elapsed().as_millis().min(i64::MAX as u128) as i64;
    // The lock is taken with a stated deadline, and a hold past its own
    // deadline is answered rather than waited out. `lock_busy` used to be the
    // only answer this function had for "somebody else has it", and that is
    // how a host spent nine and a half hours with cleanup disabled while its
    // gate said `disk_cleanup_stalled` and nothing said WHO or FOR HOW LONG.
    let pass_seconds = DEADLINE_SECONDS.max(
        registry
            .as_ref()
            .ok()
            .and_then(|data| resolve_canonical_policy(data, &report.hostname).ok())
            .and_then(|(_, policy, _, _)| policy.max_pass_seconds)
            .filter(|seconds| *seconds > 0)
            .map_or(DEADLINE_SECONDS, |seconds| seconds as f64),
    );
    let mut taken_over = false;
    let writer_label = writer.as_str().to_string();
    let lock = match acquire_lock_state(&state_dir, pass_seconds, &writer_label) {
        Ok(LockState::Held(lock)) => lock,
        Ok(LockState::TakenOver {
            lock,
            from_pid,
            overdue_seconds,
        }) => {
            taken_over = true;
            let liveness = if pid_alive(from_pid) {
                "still running and not progressing"
            } else {
                "gone"
            };
            let detail = format!(
                "took the janitor run lock from pid {from_pid} ({liveness}), {:.0}s past the \
                 deadline that holder recorded plus the {LOCK_TAKEOVER_GRACE_S:.0}s grace; this \
                 pass runs in report mode, and enforcement stays disabled until the retired \
                 predecessor inode no longer has a kernel lock",
                overdue_seconds
            );
            log_fn(&format!("disk cleanup: {detail}"));
            report.add_error("lock_taken_over", &JanitorError::os(&detail));
            lock
        }
        Ok(LockState::Busy { holder }) => {
            report.lock_busy = true;
            match holder {
                Some(holder) => {
                    let age = epoch_now() - holder.acquired_at;
                    let remaining = holder.deadline_at - epoch_now();
                    // Recognizable, not silent: an operator reading a report
                    // now learns which process holds the lock, how long it has
                    // held it and whether it is inside its own budget.
                    let detail = format!(
                        "held for {age:.0}s by pid {} ({} {}), {:.0}s of its declared budget left",
                        holder.pid,
                        holder.writer,
                        holder.writer_version,
                        remaining.max(0.0)
                    );
                    log_fn(&format!("disk cleanup: lock {detail}"));
                    report.add_error("lock_busy", &JanitorError::os(&detail));
                    report.outcome = "lock_busy".to_string();
                }
                None => {
                    // No record at all: a holder from a build older than this
                    // one, or a lock file created by hand. Say that too.
                    log_fn(
                        "disk cleanup: lock is held by a process that left no holder record; its \
                         deadline is unknown, so it will not be taken over",
                    );
                    report.add_error(
                        "lock_busy",
                        &JanitorError::os("held with no holder record; deadline unknown"),
                    );
                    report.outcome = "lock_busy_unattributed".to_string();
                }
            }
            // A busy observation must not erase the state the holder is
            // continuing. `continuing_reclaim` below relies on the previous
            // policy digest plus `pressure_active`, and the build-cache walker
            // relies on its resume cursor. Replacing those with null made the
            // next writer stop at the low watermark instead of finishing at
            // the declared target, and made every interrupted scan restart at
            // the root.
            if let Ok(previous) = read_state(&state_dir) {
                if let Some(previous) = previous.get("report").and_then(Value::as_object) {
                    report.last_success_at = previous
                        .get("last_success_at")
                        .and_then(Value::as_str)
                        .map(str::to_string);
                    report.target_name = previous
                        .get("target_name")
                        .and_then(Value::as_str)
                        .map(str::to_string);
                    report.policy_digest = previous
                        .get("policy_digest")
                        .and_then(Value::as_str)
                        .map(str::to_string);
                    report.policy_defaulted = previous
                        .get("policy_defaulted")
                        .and_then(Value::as_bool)
                        .unwrap_or(false);
                    report.mode = previous
                        .get("mode")
                        .and_then(Value::as_str)
                        .map(str::to_string);
                    report.check_interval_seconds = previous
                        .get("check_interval_seconds")
                        .and_then(Value::as_i64);
                    report.low_bytes = previous.get("low_bytes").and_then(Value::as_i64);
                    report.target_bytes = previous.get("target_bytes").and_then(Value::as_i64);
                    report.pressure_active =
                        previous.get("pressure_active").and_then(Value::as_bool);
                    report.builds_resume_from = previous
                        .get("build_caches_resume_from")
                        .and_then(Value::as_str)
                        .map(str::to_string);
                    report.unscanned_cleaners = previous
                        .get("unscanned_cleaners")
                        .and_then(Value::as_array)
                        .map(|items| {
                            items
                                .iter()
                                .filter_map(Value::as_str)
                                .map(str::to_string)
                                .collect()
                        })
                        .unwrap_or_default();
                }
            }
            if let Ok(free) = free_bytes(&home) {
                report.free_bytes_before = Some(free);
                report.free_bytes_after = Some(free);
            }
            // `persist`, not `None`: a pass prevented by a live holder is the
            // fact the stall arithmetic needs most, and without it forty
            // prevented passes and forty passes that never ran leave an
            // identical, empty record. Kept from origin/main's change to this
            // same branch of the function.
            return finish(report, started, Some(&home), persist, attempted_at, log_fn);
        }
        Err(exc) => {
            report.add_error("runtime", &exc);
            report.outcome = "invalid_or_unavailable_policy".to_string();
            return finish(report, started, Some(&home), persist, attempted_at, log_fn);
        }
    };
    let predecessor_active = match retired_locks_active(&state_dir, &lock.file) {
        Ok(active) => active,
        Err(error) => {
            report.add_error("lock_recovery", &error);
            true
        }
    };
    if predecessor_active {
        let detail =
            "a retired cleanup lock inode is still held; this pass scans and persists diagnostics but deletes nothing";
        log_fn(&format!("disk cleanup: {detail}"));
        report.add_error("lock_predecessor_active", &JanitorError::os(detail));
    }
    let lock_recovery = taken_over || predecessor_active;

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
        lock_recovery,
        log_fn,
    )
    .await
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

/// Whether free space is below the janitor's low watermark, with both numbers
/// known. The janitor's own `report.pressure_active`, asked from outside a pass.
///
/// Separate from [`disk_pressure_unresolved`] because the two answer different
/// questions and one answer for both stopped a host for seven days. Not knowing
/// the threshold is a reason to refuse admission; being under it is a reason to
/// reclaim, and on a host with nothing eligible to delete it is a state no pass
/// can leave, so it must never be the thing that silences a capacity broadcast.
pub fn disk_pressure_active(low_bytes: Option<i64>, free_bytes: Option<i64>) -> bool {
    matches!((low_bytes, free_bytes), (Some(low), Some(free)) if free < low)
}
