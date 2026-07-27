//! Per-slot lifecycle for the local GPU agent: start, heartbeat, Vast
//! pause/resume, completion, cooperative yield, status/output upload.
//!
//! Port of `stado/providers/local/slots.py`.
//!
//! Splits the single-job lifecycle out of the agent main loop so the agent
//! can manage N concurrent slots without ballooning the loop. A running
//! slot is an [`ActiveSlot`] (Python's slot dict): the live child process,
//! the [`super::Slot`] the helpers read (`job`, `pid`, `peak_vram_gb`), the
//! log-file handle, and bookkeeping timestamps.
//!
//! DEVIATIONS from the Python source (all intended):
//!   * `_write_status` / `_write_heartbeat` / `_upload_output` raise
//!     `RuntimeError` in Python when `store._sdk_bucket is None`; every Rust
//!     [`JobStorage`] has a blob backend that can write text, so no such
//!     gate exists (writes go through the backend, SDK or local file).
//!   * The job subprocess runs under `/bin/sh -c` (Python `shell=True`,
//!     which is `/bin/sh` on POSIX) with `process_group(0)` — the exact
//!     semantic of Python's `start_new_session=True` (setsid; the child
//!     becomes a process-group leader so `killpg` can reap the whole tree).
//!   * The optional output mirror accepts only `stado://` object prefixes
//!     and writes through [`JobStorage`]. Mirror failure is logged because
//!     canonical `status/<id>/output/` data is already durable.
//!   * `WISENT_RAW_*` env floats that fail to parse panic (Python's
//!     `float()` raises `ValueError`, crashing the agent visibly).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use chrono::Utc;
use nix::sys::signal::Signal;
use nix::unistd::Pid;
use serde_json::Value;

use crate::constants;
use crate::models::{
    activation_extraction_must_share_gpu, deprecated_activation_command_reason, isoformat_utc,
    job_state, Job,
};
use crate::queue::{JobStorage, StorageError};
use crate::sizing::Sizing;

use super::gpu_probe;
use super::helpers;
use super::{build_job_command, verify_command, Slot};

/// Write a fresh heartbeat every 60s; HEARTBEAT_STALE_MINUTES=15 leaves 15
/// missed-write tolerance. Python `HEARTBEAT_INTERVAL`.
pub const HEARTBEAT_INTERVAL_S: u64 = constants::SLOT_HEARTBEAT_INTERVAL_S;

/// Python `Job.yield_grace_seconds` fallback (`getattr(...) or 120`).
const DEFAULT_YIELD_GRACE_S: i64 = 120;
/// Python `Job.max_yields_before_protected` fallback (`getattr(...) or 5`).
pub const DEFAULT_MAX_YIELDS: i64 = 5;

/// A running local-agent slot (Python's slot dict). Owns the child process
/// handle; dropping it without reaping leaves the OS process running (the
/// child is in its own process group and re-parents to init), matching the
/// Python agent dropping a `Popen` handle.
pub struct ActiveSlot {
    /// The helper-visible slot state (`job`, `pid`, `peak_vram_gb`).
    pub slot: Slot,
    child: tokio::process::Child,
    /// Our copy of the log-file handle (stdout/stderr were dup'd from it).
    /// `None` after close — Python's flush+close-once discipline.
    log_file: Option<std::fs::File>,
    /// Last heartbeat stamp (monotonic). Python `last_hb` (time.time()).
    pub last_hb: Instant,
    /// Currently SIGSTOPed because a Vast renter appeared.
    pub paused: bool,
    /// Spawn time (monotonic), for the MIN_RUNTIME_BEFORE_YIELD_S guard.
    pub started_mono: Instant,
    /// Detached heartbeat task (daemon-thread parity); exits when the pid dies.
    _hb_task: tokio::task::JoinHandle<()>,
    /// Shared hold on the janitor's cleanup lock for this live workload
    /// (Python `slot["disk_cleanup_lock"]`). Released when the slot is
    /// dropped (flock closes with the fd) or explicitly via
    /// [`super::disk_cleanup::release_workload_lock`].
    pub disk_cleanup_lock: Option<super::disk_cleanup::WorkloadLock>,
}

impl std::fmt::Debug for ActiveSlot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ActiveSlot")
            .field("job_id", &self.slot.job.job_id)
            .field("pid", &self.slot.pid)
            .field("paused", &self.paused)
            .finish()
    }
}

impl ActiveSlot {
    /// Root pid of the job's `sh -c <cmd>` process; also the process-group
    /// id (spawned with `process_group(0)`).
    pub fn pid(&self) -> i32 {
        self.slot
            .pid
            .expect("ActiveSlot always has a pid after spawn")
    }

    /// Python `slot["log_file"].flush(); slot["log_file"].close()`. The
    /// agent keeps no userspace buffer on this file (the child's writes go
    /// straight to the dup'd fd), so closing our copy is the whole flush —
    /// what matters is that it happens BEFORE the output upload reads the
    /// file from disk (see the 2026-05-06 zero-byte-log incident below).
    fn close_log(&mut self) {
        self.log_file.take();
    }
}

/// The result of one [`advance_slot`] tick (Python's bool return: True =
/// still running).
// ActiveSlot is large (owns the Job + child handle), but the enum moves
// once per tick per slot — boxing would buy nothing measurable.
#[allow(clippy::large_enum_variant)]
pub enum SlotOutcome {
    Running(ActiveSlot),
    /// Completed, failed, OOM-escalated, or dropped as a duplicate.
    Done,
}

// ---------------------------------------------------------------------------
// status / heartbeat writes
// ---------------------------------------------------------------------------

/// Python `_write_status`. Refusal-without-backend collapsed away (see the
/// module-docs deviation): every Rust backend can write the blob.
pub async fn write_status(
    store: &JobStorage,
    job_id: &str,
    status: &str,
) -> Result<(), StorageError> {
    store
        .upload_text(&format!("status/{job_id}/status"), status)
        .await
}

/// Stamp a fresh `status/<job_id>/heartbeat` blob so the CF monitor sees
/// the workstation slot is alive. Python `_write_heartbeat`.
///
/// Earlier this used `subprocess.run([gsutil, cp, ...], capture_output=True)`
/// which silently swallowed any failure. When gsutil hit a transient auth
/// glitch, network blip, or concurrent-fork ENOMEM, the heartbeat write
/// vanished into the void; the CF monitor saw an old/missing blob, and
/// requeued every workstation job at the 15-minute staleness threshold.
/// Confirmed live on 2026-05-06 (job 01d79e28 had no heartbeat blob despite
/// the slot being live; jobs 4724ae6d/3f16d8b4/24dee60d were yanked from
/// running/ for 'stale heartbeat (local consumer)' in a single 4-second
/// monitor window).
///
/// Writes go through the storage backend directly (no fork, no swallowed
/// error).
pub async fn write_heartbeat(store: &JobStorage, job_id: &str) -> Result<(), StorageError> {
    let ts = isoformat_utc(Utc::now());
    store
        .upload_text(
            &format!("status/{job_id}/heartbeat"),
            &format!("RUNNING {ts}"),
        )
        .await
}

/// Stamp status/<job>/heartbeat every HEARTBEAT_INTERVAL_S for as long as
/// the training subprocess is alive — independent of the agent main loop.
/// Python `_start_heartbeat_thread`.
///
/// The loop-coupled write (slots tick, only fires when the agent reaches
/// it) let a loop blocked >1800s on another slot's checkpoint pull / drift
/// check / HF download starve a HEALTHY job's heartbeat, so the CF monitor
/// orphan-requeued it: Llama 3ef705b2 + Qwen3 724084db were both requeued
/// in one monitor pass at 2026-05-15T16:18:42 ('local agent live but job
/// heartbeat stale (orphan)') while training was actively progressing. A
/// task keyed on pid liveness makes the heartbeat mean 'training process
/// alive', not 'agent loop ran recently'.
pub fn start_heartbeat_task(
    store: JobStorage,
    job_id: String,
    pid: i32,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        while helpers::pid_alive(pid) {
            tokio::time::sleep(Duration::from_secs(HEARTBEAT_INTERVAL_S)).await;
            if let Err(err) = write_heartbeat(&store, &job_id).await {
                // The coordinator requeues local jobs when their heartbeat
                // goes stale. Silent heartbeat failures leave live jobs looking
                // dead, so make the next failure visible in the agent log.
                eprintln!("[heartbeat] write failed for {job_id}: {err}");
            }
        }
    })
}

// ---------------------------------------------------------------------------
// output upload + log tail
// ---------------------------------------------------------------------------

/// All regular files under `dir`, recursively (Python `Path.rglob("*")`
/// filtered to `is_file`). Order is readdir order, like Python's scandir
/// order.
fn walk_files(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            out.extend(walk_files(&path));
        } else if path.is_file() {
            out.push(path);
        }
    }
    out
}

/// Upload every regular file under output_dir to `status/<job_id>/output/`.
/// Python `_upload_output`.
///
/// Earlier this used `subprocess.run([gsutil, -m, cp, -r, ..., capture_output=True])`
/// which silently swallowed failures (same fire-and-forget gsutil pattern as
/// the heartbeat bug). On the workstation, this resulted in 7/7 sampled
/// `local@ubuntu-server` completions on 2026-05-07 having NO log file at all
/// in GCS (`gsutil cat` returned `CommandException: No URLs matched`).
/// Backend-based upload raises on failure; the caller decides whether that
/// is fatal (advance_slot: yes; request_yield: logged, non-fatal).
pub async fn upload_output(
    store: &JobStorage,
    job_id: &str,
    output_dir: &Path,
) -> Result<(), StorageError> {
    if !output_dir.exists() {
        return Ok(());
    }
    for path in walk_files(output_dir) {
        let rel = path
            .strip_prefix(output_dir)
            .unwrap_or(&path)
            .components()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join("/");
        let bytes = tokio::fs::read(&path).await?;
        store
            .upload_bytes(&format!("status/{job_id}/output/{rel}"), &bytes)
            .await?;
    }
    Ok(())
}

/// Last max_bytes of the per-job log; "" if missing. Python `_tail_log`.
pub fn tail_log(path: &Path, max_bytes: u64) -> String {
    let Ok(data) = std::fs::read(path) else {
        return String::new();
    };
    let start = data.len().saturating_sub(max_bytes as usize);
    String::from_utf8_lossy(&data[start..]).trim().to_string()
}

// ---------------------------------------------------------------------------
// small env / json / process helpers
// ---------------------------------------------------------------------------

/// Python `float(os.environ.get(key, default) or default)`: unset or empty
/// -> default; otherwise float() (whitespace-tolerant). A non-numeric value
/// panics — Python's ValueError crashes the agent at exactly this spot.
fn env_f64(key: &str, default: f64) -> f64 {
    match std::env::var(key) {
        Ok(raw) if !raw.is_empty() => raw
            .trim()
            .parse()
            .unwrap_or_else(|_| panic!("{key} must be a float (Python float() parity): {raw}")),
        _ => default,
    }
}

/// Recursively key-sorted compact JSON (Python
/// `json.dumps(d, sort_keys=True, separators=(",", ":"))`, ensure_ascii=True).
pub fn canonical_json(value: &Value) -> String {
    crate::models::ensure_ascii(
        &serde_json::to_string(&sort_keys(value)).expect("Value serialization is infallible"),
    )
}

fn sort_keys(value: &Value) -> Value {
    match value {
        Value::Object(map) => {
            let mut entries: Vec<(&String, &Value)> = map.iter().collect();
            entries.sort_by(|a, b| a.0.cmp(b.0));
            Value::Object(
                entries
                    .into_iter()
                    .map(|(k, v)| (k.clone(), sort_keys(v)))
                    .collect(),
            )
        }
        Value::Array(items) => Value::Array(items.iter().map(sort_keys).collect()),
        other => other.clone(),
    }
}

/// Python `Popen.returncode`: the exit code, or `-signum` when the child
/// was killed by a signal.
pub fn python_returncode(status: std::process::ExitStatus) -> i32 {
    use std::os::unix::process::ExitStatusExt;
    match status.signal() {
        Some(sig) => -sig,
        None => status.code().unwrap_or(0),
    }
}

/// Python `(res.stderr or res.stdout or "")[:n]` on captured bytes.
fn captured_head(stderr: &[u8], stdout: &[u8], n: usize) -> String {
    let text = if !stderr.is_empty() {
        String::from_utf8_lossy(stderr).into_owned()
    } else if !stdout.is_empty() {
        String::from_utf8_lossy(stdout).into_owned()
    } else {
        String::new()
    };
    text.chars().take(n).collect()
}

/// Last `n` chars (Python `s[-n:]`).
fn tail_chars(s: &str, n: usize) -> String {
    s.chars()
        .rev()
        .take(n)
        .collect::<String>()
        .chars()
        .rev()
        .collect()
}

/// First `n` chars (Python `s[:n]`).
fn head_chars(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

/// Copy only execution-runtime variables into untrusted job subprocesses.
/// Control-plane config, Skarbiec routing, cloud credentials, and storage
/// locators stay exclusively in the Stado agent process.
fn inherit_safe_agent_environment(command: &mut tokio::process::Command) {
    const EXACT: &[&str] = &[
        "PATH",
        "HOME",
        "USER",
        "LOGNAME",
        "SHELL",
        "LANG",
        "LC_ALL",
        "LC_CTYPE",
        "TZ",
        "TMPDIR",
        "VIRTUAL_ENV",
        "CONDA_PREFIX",
        "PYTHONPATH",
        "LD_LIBRARY_PATH",
        "SSL_CERT_FILE",
        "SSL_CERT_DIR",
        "REQUESTS_CA_BUNDLE",
        "CURL_CA_BUNDLE",
        "HTTP_PROXY",
        "HTTPS_PROXY",
        "NO_PROXY",
    ];
    const PREFIXES: &[&str] = &["CUDA_", "NVIDIA_", "HIP_", "ROCR_", "WISENT_RAW_"];
    command.env_clear();
    for (name, value) in std::env::vars_os() {
        let key = name.to_string_lossy();
        if EXACT.contains(&key.as_ref()) || PREFIXES.iter().any(|prefix| key.starts_with(prefix)) {
            command.env(name, value);
        }
    }
}

fn valid_env_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|ch| ch == '_' || ch.is_ascii_alphabetic())
        && chars.all(|ch| ch == '_' || ch.is_ascii_alphanumeric())
}

async fn resolve_job_secret_environment(
    job: &Job,
) -> Result<BTreeMap<String, String>, StorageError> {
    let mut environment = BTreeMap::new();
    if job.secret_env.is_empty() {
        return Ok(environment);
    }
    let agent_token_file = crate::config::agent_skarbiec_token_file();
    let client = if Path::new(agent_token_file).is_file() {
        let configured_url = crate::config::agent_skarbiec_url();
        let url = if configured_url.trim().is_empty() {
            crate::config::skarbiec_url()
        } else {
            configured_url
        };
        crate::skarbiec::Client::new(
            url,
            crate::config::agent_skarbiec_consumer(),
            agent_token_file,
        )
    } else if crate::config::skarbiec_consumer().ends_with("-agent") {
        crate::skarbiec::Client::configured()
    } else {
        return Err(StorageError::Other(
            "workload secrets require a dedicated agent Skarbiec grant".to_string(),
        ));
    }
    .map_err(|error| {
        StorageError::Other(format!(
            "cannot configure workload secret resolver: {error}"
        ))
    })?;
    for (env_name, reference) in &job.secret_env {
        if !valid_env_name(env_name)
            || reference.item.trim().is_empty()
            || reference.field.trim().is_empty()
        {
            return Err(StorageError::Other(format!(
                "job {} contains an invalid secret environment reference",
                job.job_id
            )));
        }
        let value = client
            .read_string(&reference.item, &reference.field)
            .await
            .map_err(|error| {
                StorageError::Other(format!(
                    "cannot resolve job {} secret {env_name}: {error}",
                    job.job_id
                ))
            })?
            .filter(|value| !value.is_empty())
            .ok_or_else(|| {
                StorageError::Other(format!(
                    "job {} secret {env_name} is missing or empty",
                    job.job_id
                ))
            })?;
        environment.insert(env_name.clone(), value);
    }
    Ok(environment)
}

// ---------------------------------------------------------------------------
// admission helpers
// ---------------------------------------------------------------------------

/// Python `_raw_active_disk_refusal`: refuse a raw-activation job when the
/// pending-staging root can't guarantee the reserve + headroom.
pub fn raw_active_disk_refusal(command: &str) -> String {
    if !activation_extraction_must_share_gpu(command) {
        return String::new();
    }
    let tmpdir = std::env::var("TMPDIR").unwrap_or_else(|_| "/tmp".to_string());
    let root = Path::new(&tmpdir).join("wisent_raw_pending");
    let free_gb = match std::fs::create_dir_all(&root).and_then(|()| {
        nix::sys::statvfs::statvfs(&root).map_err(|e| std::io::Error::from_raw_os_error(e as i32))
    }) {
        Ok(stat) => stat.blocks_available() as f64 * stat.fragment_size() as f64 / 1024f64.powi(3),
        Err(exc) => return format!("raw active root unavailable: {}: {exc}", root.display()),
    };
    let reserve = env_f64("WISENT_RAW_CLAIM_RESERVE_GB", 180.0);
    let min_free = match std::env::var("WISENT_RAW_CLAIM_MIN_FREE_GB") {
        Ok(raw) if !raw.is_empty() => raw.trim().parse().unwrap_or_else(|_| {
            panic!("WISENT_RAW_CLAIM_MIN_FREE_GB must be a float (Python float() parity): {raw}")
        }),
        _ => env_f64("WISENT_RAW_HOT_FREE_TARGET_GB", 270.0),
    };
    if free_gb - reserve < min_free {
        return format!(
            "raw active staging low: {} free={free_gb:.1}GB reserve={reserve:.1}GB min_free={min_free:.1}GB",
            root.display()
        );
    }
    String::new()
}

/// Python's `repr(list)` for a string list: `['a', 'b']` (package names
/// never contain quotes in practice).
fn py_list_repr(items: &[String]) -> String {
    let inner = items
        .iter()
        .map(|i| format!("'{i}'"))
        .collect::<Vec<_>>()
        .join(", ");
    format!("[{inner}]")
}

/// Install job.apt_packages via sudo apt-get on cloud-kind agents.
/// Python `_install_apt_packages`.
///
/// Returns true on success (or no-op when no packages were requested),
/// false on failure. The caller refuses to start the slot when this
/// returns false so the job stays queued for retry instead of running
/// against missing system deps and failing with a confusing error.
///
/// Refuses on kind='local' (physical operator workstation) — the
/// operator owns what's installed on their box, and silent
/// sudo-apt-installs from queued jobs are a footgun. Cloud VMs
/// (kind='gcp'/'azure'/'aws') run with passwordless sudo by default
/// on the deeplearning-platform image, so apt-install Just Works.
pub async fn install_apt_packages(job: &Job, kind: &str, log_fn: &mut dyn FnMut(&str)) -> bool {
    if job.apt_packages.is_empty() {
        return true;
    }
    if kind == "local" {
        log_fn(&format!(
            "refuse {}: apt_packages={} requested but agent kind=local; \
             install manually or submit to a cloud-kind agent",
            job.job_id,
            py_list_repr(&job.apt_packages)
        ));
        return false;
    }
    log_fn(&format!(
        "apt-install for {}: {}",
        job.job_id,
        job.apt_packages.join(" ")
    ));
    let res = tokio::process::Command::new("sudo")
        .args(["-n", "apt-get", "install", "-y", "--no-install-recommends"])
        .args(&job.apt_packages)
        .output()
        .await;
    match res {
        Ok(out) if out.status.success() => true,
        Ok(out) => {
            log_fn(&format!(
                "apt-install FAILED for {}: rc={} err={}",
                job.job_id,
                python_returncode(out.status),
                captured_head(&out.stderr, &out.stdout, 200)
            ));
            false
        }
        Err(exc) => {
            log_fn(&format!(
                "apt-install FAILED for {}: spawn error: {exc}",
                job.job_id
            ));
            false
        }
    }
}

/// Copy every output file to the caller's provider-neutral Stado object
/// prefix. Additive — the canonical status path was already uploaded.
///
/// Failures remain non-fatal because canonical output is durable and the
/// caller can re-run the mirror without changing job lifecycle state.
pub async fn mirror_to_output_uri(store: &JobStorage, job: &Job, log_fn: &mut dyn FnMut(&str)) {
    let uri = job.output_uri.trim();
    if uri.is_empty() {
        return;
    }
    let base = match crate::object_store::ObjectRef::parse(uri) {
        Ok(base) => base,
        Err(error) => {
            log_fn(&format!(
                "output_uri mirror refused for {}: {error}",
                job.job_id
            ));
            return;
        }
    };
    let output_dir = PathBuf::from(format!("/tmp/wc-{}/output", job.job_id));
    if !output_dir.exists() {
        return;
    }
    for path in walk_files(&output_dir) {
        let relative = path
            .strip_prefix(&output_dir)
            .unwrap_or(&path)
            .components()
            .map(|component| component.as_os_str().to_string_lossy())
            .collect::<Vec<_>>()
            .join("/");
        let object = match crate::object_store::ObjectRef::new(
            base.namespace(),
            &format!("{}/{relative}", base.key().trim_end_matches('/')),
        ) {
            Ok(object) => object,
            Err(error) => {
                log_fn(&format!("output_uri mirror path failed: {error}"));
                continue;
            }
        };
        match tokio::fs::read(&path).await {
            Ok(content) => {
                if let Err(error) = store.upload_bytes(&object.storage_path(), &content).await {
                    log_fn(&format!(
                        "output_uri mirror failed for {} -> {object}: {error}",
                        job.job_id
                    ));
                }
            }
            Err(error) => log_fn(&format!(
                "output_uri mirror failed to read {}: {error}",
                path.display()
            )),
        }
    }
}

/// Materialize explicitly declared Stado objects while storage credentials
/// are still confined to the trusted agent process.
async fn materialize_stado_inputs(
    store: &JobStorage,
    inputs: &serde_json::Map<String, Value>,
    work_dir: &Path,
) -> Result<(), StorageError> {
    use sha2::Digest as _;
    fn reject_symlink(path: &Path) -> Result<(), StorageError> {
        match std::fs::symlink_metadata(path) {
            Ok(metadata) if metadata.file_type().is_symlink() => Err(StorageError::PathEscape(
                format!("job input path contains a symlink: {}", path.display()),
            )),
            Ok(_) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(StorageError::Io(error)),
        }
    }

    for (name, value) in inputs {
        let Some(spec) = value.as_object() else {
            continue;
        };
        let Some(uri) = spec.get("stado_uri").and_then(Value::as_str) else {
            continue;
        };
        let relative = spec
            .get("relative_path")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                StorageError::Other(format!(
                    "input {name} with stado_uri requires relative_path"
                ))
            })?;
        let relative_path = Path::new(relative);
        if relative_path.as_os_str().is_empty()
            || relative_path
                .components()
                .any(|part| !matches!(part, std::path::Component::Normal(_)))
        {
            return Err(StorageError::Other(format!(
                "input {name} relative_path must stay inside the job work directory"
            )));
        }
        let object = crate::object_store::ObjectRef::parse(uri)?;
        let content = store
            .read_bytes(&object.storage_path())
            .await?
            .ok_or_else(|| StorageError::Other(format!("input {name} is absent: {object}")))?;
        if let Some(expected) = spec
            .get("sha256")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
        {
            let actual = hex::encode(sha2::Sha256::digest(&content));
            if actual != expected {
                return Err(StorageError::Other(format!(
                    "input {name} digest mismatch: expected {expected}, got {actual}"
                )));
            }
        }
        let destination = work_dir.join(relative_path);
        let mut checked = work_dir.to_path_buf();
        reject_symlink(&checked)?;
        for component in relative_path.components() {
            let std::path::Component::Normal(part) = component else {
                unreachable!("relative path was validated above");
            };
            checked.push(part);
            reject_symlink(&checked)?;
        }
        if let Some(parent) = destination.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        tokio::fs::write(destination, content).await?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// start_slot
// ---------------------------------------------------------------------------

/// Spawn a subprocess for `job`, register it in 'running' state, return the
/// slot. Python `start_slot`.
///
/// Returns None when a dedupe/refusal check fires or apt-install refuses —
/// the caller leaves the job in queue/ for another agent to claim (or the
/// job was already moved/dropped by the check itself).
pub async fn start_slot(
    store: &JobStorage,
    mut job: Job,
    hostname: &str,
    log_fn: &mut dyn FnMut(&str),
    kind: &str,
) -> Result<Option<ActiveSlot>, StorageError> {
    let cmd = job.command.clone();
    if activation_extraction_must_share_gpu(&cmd) {
        job.exclusive = false;
    }
    for terminal_prefix in ["uploaded", "completed", "cancelled"] {
        if store
            .read_job(terminal_prefix, &job.job_id)
            .await?
            .is_some()
        {
            store.delete_job("queue", &job.job_id).await?;
            log_fn(&format!(
                "drop duplicate queued {}: already in {terminal_prefix}/",
                job.job_id
            ));
            return Ok(None);
        }
    }
    let reason = deprecated_activation_command_reason(&cmd);
    if !reason.is_empty() {
        job.state = job_state::FAILED.to_string();
        job.failed_at = Some(isoformat_utc(Utc::now()));
        job.error = Some(reason.to_string());
        store.move_job(&job, "queue", "failed").await?;
        log_fn(&format!("refuse {}: {reason}", job.job_id));
        return Ok(None);
    }
    if !install_apt_packages(&job, kind, log_fn).await {
        return Ok(None);
    }
    let raw_refusal = raw_active_disk_refusal(&cmd);
    if !raw_refusal.is_empty() {
        log_fn(&format!("refuse {}: {raw_refusal}", job.job_id));
        return Ok(None);
    }
    let work_dir = format!("/tmp/wc-{}", job.job_id);
    std::fs::create_dir_all(format!("{work_dir}/output"))?;
    let artifact_inputs_json = canonical_json(&Value::Object(job.resolved_input_artifacts.clone()));
    let artifact_inputs_file = format!("{work_dir}/artifacts.json");
    std::fs::write(&artifact_inputs_file, &artifact_inputs_json)?;
    if let Err(error) =
        materialize_stado_inputs(store, &job.resolved_input_artifacts, Path::new(&work_dir)).await
    {
        job.state = job_state::FAILED.to_string();
        job.failed_at = Some(isoformat_utc(Utc::now()));
        job.error = Some(format!("input materialization failed: {error}"));
        store.move_job(&job, "queue", "failed").await?;
        log_fn(&format!("refuse {}: {error}", job.job_id));
        return Ok(None);
    }
    let secret_environment = match resolve_job_secret_environment(&job).await {
        Ok(environment) => environment,
        Err(error) => {
            job.state = job_state::FAILED.to_string();
            job.failed_at = Some(isoformat_utc(Utc::now()));
            job.error = Some(format!("workload secret resolution failed: {error}"));
            store.move_job(&job, "queue", "failed").await?;
            log_fn(&format!("refuse {}: {error}", job.job_id));
            return Ok(None);
        }
    };
    write_status(
        store,
        &job.job_id,
        &format!("RUNNING {}", isoformat_utc(Utc::now())),
    )
    .await?;
    job.state = job_state::RUNNING.to_string();
    job.started_at = Some(isoformat_utc(Utc::now()));
    job.instance_ref = Some(format!("local@{hostname}"));
    store.move_job(&job, "queue", "running").await?;
    let log_file = std::fs::File::create(format!("{work_dir}/output/command_output.log"))?;
    let full_command = build_job_command(&job);
    // WISENT_FLEET_STAGING_DIR points at a persistent agent-owned staging
    // dir. wisent's upload_extracted_activations writes shards there and
    // SKIPS the per-job flush. The agent flushes the whole dir periodically
    // across ALL jobs as one HF commit — reduces 429 risk drastically.
    let fleet_staging = std::env::var("WISENT_FLEET_STAGING_DIR").unwrap_or_else(|_| {
        let tmpdir = std::env::var("TMPDIR").unwrap_or_else(|_| "/tmp".to_string());
        Path::new(&tmpdir)
            .join("wisent_fleet_staging")
            .to_string_lossy()
            .into_owned()
    });
    std::fs::create_dir_all(&fleet_staging)?;
    let mut command = tokio::process::Command::new("/bin/sh");
    inherit_safe_agent_environment(&mut command);
    command
        .arg("-c")
        .arg(&full_command)
        .current_dir(&work_dir)
        .env("WISENT_DTYPE", "auto")
        .env("PYTHONUNBUFFERED", "1")
        .env("HF_HUB_DISABLE_XET", "1")
        .env("WISENT_FLEET_STAGING_DIR", &fleet_staging)
        .env("WC_JOB_ID", &job.job_id)
        .env("WC_ARTIFACT_INPUTS_JSON", &artifact_inputs_json)
        .env("WC_ARTIFACT_INPUTS_FILE", &artifact_inputs_file)
        .envs(secret_environment)
        .stdout(std::process::Stdio::from(log_file.try_clone()?))
        // subprocess.STDOUT parity: stderr lands in the same log file.
        .stderr(std::process::Stdio::from(log_file.try_clone()?))
        // Own session/process group so a cooperative yield (request_yield)
        // can signal the WHOLE job tree via the group, and SIGKILL it
        // cleanly if the grace is blown — without that, killing only the
        // shell pid would orphan the GPU process and never free its VRAM.
        // The existing Vast SIGSTOP/SIGCONT still target the root pid
        // directly, so their behavior is unchanged.
        .process_group(0);
    let child = command.spawn()?;
    let pid = child.id().expect("freshly spawned child has a pid") as i32;
    log_fn(&format!(
        "Started job {}: {}",
        job.job_id,
        head_chars(&job.command, 60)
    ));
    write_heartbeat(store, &job.job_id).await?;
    let hb_task = start_heartbeat_task(store.clone(), job.job_id.clone(), pid);
    let slot = Slot {
        job,
        pid: Some(pid),
        peak_vram_gb: 0,
    };
    Ok(Some(ActiveSlot {
        slot,
        child,
        log_file: Some(log_file),
        last_hb: Instant::now(),
        paused: false,
        started_mono: Instant::now(),
        _hb_task: hb_task,
        disk_cleanup_lock: None,
    }))
}

// ---------------------------------------------------------------------------
// request_yield
// ---------------------------------------------------------------------------

/// Cooperatively yield a running slot to free its VRAM for higher-priority
/// work. Returns true once the job has been requeued.
/// Python `request_yield`.
///
/// Sequence (total bounded by job.yield_grace_seconds):
///   1. Run the job's yield_command (the save-and-sync hook) in the job
///      workdir with WC_JOB_PID set to the process-group leader, so the hook
///      can signal the job, persist state + artifacts externally, and let it
///      exit.
///   2. Wait for the process to exit on its own within the remaining grace.
///   3. SIGKILL the whole process group only if the grace is blown (logged
///      loudly — a timed-out yield means the hook didn't actually stop it).
///   4. Requeue: running -> queue, state QUEUED, yield_count++, clear
///      instance_ref/started_at. NOT marked FAILED — resume is the job's own
///      business (checkpoint pull, server-side state, ...).
///
/// The slot's process was started with process_group(0), so its pid is the
/// process-group id.
pub async fn request_yield(
    mut slot: ActiveSlot,
    store: &JobStorage,
    log_fn: &mut dyn FnMut(&str),
) -> Result<bool, StorageError> {
    let pgid = slot.pid();
    let mut job = slot.slot.job.clone();
    // Python `int(getattr(job, "yield_grace_seconds", 120) or 120)`: 0 -> 120.
    let grace = if job.yield_grace_seconds != 0 {
        job.yield_grace_seconds
    } else {
        DEFAULT_YIELD_GRACE_S
    };
    let hook = job.yield_command.trim().to_string();
    let work_dir = format!("/tmp/wc-{}", job.job_id);
    let deadline = Instant::now() + Duration::from_secs(grace.max(0) as u64);
    log_fn(&format!(
        "yield: requesting yield of {} (grace={grace}s, pgid={pgid})",
        job.job_id
    ));

    if !hook.is_empty() {
        let remaining = deadline
            .saturating_duration_since(Instant::now())
            .as_secs()
            .max(1);
        let secret_environment = match resolve_job_secret_environment(&job).await {
            Ok(environment) => environment,
            Err(error) => {
                log_fn(&format!(
                    "yield: workload secret resolution failed for {}: {error}",
                    job.job_id
                ));
                BTreeMap::new()
            }
        };
        let mut cmd = tokio::process::Command::new("/bin/sh");
        inherit_safe_agent_environment(&mut cmd);
        cmd.arg("-c")
            .arg(&hook)
            .env("WC_JOB_ID", &job.job_id)
            .env("WC_JOB_PID", pgid.to_string())
            .envs(secret_environment)
            // A timed-out hook is killed (Python subprocess.run timeout
            // semantics: kill the direct child, reap, raise).
            .kill_on_drop(true);
        if Path::new(&work_dir).exists() {
            cmd.current_dir(&work_dir);
        }
        match tokio::time::timeout(Duration::from_secs(remaining), cmd.output()).await {
            Ok(Ok(out)) => {
                let rc = python_returncode(out.status);
                if rc != 0 {
                    log_fn(&format!(
                        "yield: on-yield hook {} rc={rc}: {}",
                        job.job_id,
                        captured_head(&out.stderr, &out.stdout, 200)
                    ));
                }
            }
            Ok(Err(exc)) => log_fn(&format!(
                "yield: on-yield hook {} raised: {exc}",
                job.job_id
            )),
            Err(_) => log_fn(&format!(
                "yield: on-yield hook {} exceeded grace; terminating",
                job.job_id
            )),
        }
    }

    loop {
        if slot.child.try_wait()?.is_some() {
            break;
        }
        if Instant::now() >= deadline {
            break;
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    if slot.child.try_wait()?.is_none() {
        log_fn(&format!(
            "yield: {} still alive after grace — SIGKILL group {pgid}",
            job.job_id
        ));
        // ProcessLookupError parity: the group may have exited between the
        // poll and the signal.
        let _ = nix::sys::signal::killpg(Pid::from_raw(pgid), Signal::SIGKILL);
        let _ = tokio::time::timeout(Duration::from_secs(10), slot.child.wait()).await;
    }

    slot.close_log();

    job.yield_count += 1;
    job.state = job_state::QUEUED.to_string();
    job.instance_ref = None;
    job.started_at = None;
    write_status(
        store,
        &job.job_id,
        &format!("YIELDED {}", isoformat_utc(Utc::now())),
    )
    .await?;
    let output_dir = format!("/tmp/wc-{}/output", job.job_id);
    if Path::new(&output_dir).exists() {
        if let Err(exc) = upload_output(store, &job.job_id, Path::new(&output_dir)).await {
            log_fn(&format!(
                "yield: output upload {} failed (non-fatal): {exc}",
                job.job_id
            ));
        }
    }
    // running -> queue (NOT a terminal state, so the tracking tombstone hook
    // is a no-op and the CF monitor leaves it alone once out of running/).
    store.move_job(&job, "running", "queue").await?;
    log_fn(&format!(
        "yield: {} requeued (yield_count={})",
        job.job_id, job.yield_count
    ));
    Ok(true)
}

// ---------------------------------------------------------------------------
// advance_slot
// ---------------------------------------------------------------------------

/// Advance one slot. Python `advance_slot` — returns [`SlotOutcome::Running`]
/// while the job runs, [`SlotOutcome::Done`] once completed/failed/dropped.
pub async fn advance_slot(
    mut slot: ActiveSlot,
    store: &JobStorage,
    sizing: &Sizing,
    vast_active: bool,
    log_fn: &mut dyn FnMut(&str),
) -> Result<SlotOutcome, StorageError> {
    let pid = slot.pid();
    let job_id = slot.slot.job.job_id.clone();
    for terminal_prefix in ["uploaded", "completed", "cancelled"] {
        if store.read_job(terminal_prefix, &job_id).await?.is_some() {
            // proc.terminate(): SIGTERM to the root shell pid only (NOT the
            // process group — matches Python exactly).
            let _ = nix::sys::signal::kill(Pid::from_raw(pid), Signal::SIGTERM);
            slot.close_log();
            let _ = store.delete_job("running", &job_id).await;
            log_fn(&format!(
                "drop duplicate running {job_id}: already in {terminal_prefix}/"
            ));
            return Ok(SlotOutcome::Done);
        }
    }
    if !slot.paused && vast_active {
        log_fn(&format!("Renter detected, pausing job {job_id}"));
        nix::sys::signal::kill(Pid::from_raw(pid), Signal::SIGSTOP)
            .map_err(|e| StorageError::Other(format!("SIGSTOP pid {pid}: {e}")))?;
        slot.paused = true;
    } else if slot.paused && !vast_active {
        log_fn(&format!("Renter gone, resuming job {job_id}"));
        nix::sys::signal::kill(Pid::from_raw(pid), Signal::SIGCONT)
            .map_err(|e| StorageError::Other(format!("SIGCONT pid {pid}: {e}")))?;
        slot.paused = false;
    }
    let Some(exit_status) = slot.child.try_wait()? else {
        // Still running: refresh the peak-VRAM attribution and, on the
        // heartbeat interval, the status blob + streamed log.
        let used = gpu_probe::smi_job_used_gb(pid).await;
        if used > slot.slot.peak_vram_gb {
            slot.slot.peak_vram_gb = used;
        }
        if !slot.paused && slot.last_hb.elapsed() > Duration::from_secs(HEARTBEAT_INTERVAL_S) {
            write_heartbeat(store, &job_id).await?;
            // Stream the in-progress command_output.log to storage on each
            // heartbeat. Without this, a job killed mid-run leaves its log
            // on the workstation /tmp dir and the operator has zero crash
            // evidence.
            let log_path = format!("/tmp/wc-{job_id}/output/command_output.log");
            if Path::new(&log_path).exists() {
                let upload = async {
                    let bytes = tokio::fs::read(&log_path).await?;
                    store
                        .upload_bytes(
                            &format!("status/{job_id}/output/command_output.log"),
                            &bytes,
                        )
                        .await
                };
                match tokio::time::timeout(Duration::from_secs(10), upload).await {
                    Ok(Ok(())) => {}
                    Ok(Err(exc)) => {
                        log_fn(&format!(
                            "heartbeat log upload failed for {job_id}: {}",
                            head_chars(&exc.to_string(), 160)
                        ));
                    }
                    Err(_) => {
                        log_fn(&format!(
                            "heartbeat log upload failed for {job_id}: timed out after 10s"
                        ));
                    }
                }
            }
            slot.last_hb = Instant::now();
        }
        return Ok(SlotOutcome::Running(slot));
    };

    let mut ret = python_returncode(exit_status);
    let mut verify_err = String::new();
    let verify_cmd = verify_command(&slot.slot.job);
    if ret == 0 && !verify_cmd.is_empty() {
        // Verification hook — see Job.verify_command docstring. Runs in
        // the same workdir as the original command. Non-zero exit
        // reverses the COMPLETED→FAILED. The verify command must define
        // its own clear failure conditions; the runner does not impose
        // a wall-clock cap.
        let secret_environment = match resolve_job_secret_environment(&slot.slot.job).await {
            Ok(environment) => environment,
            Err(error) => {
                ret = i32::MAX;
                verify_err = format!("verify_command secret resolution failed: {error}");
                log_fn(&format!(
                    "verify_command secret resolution failed for {job_id}: {error}"
                ));
                BTreeMap::new()
            }
        };
        let mut command = tokio::process::Command::new("/bin/sh");
        inherit_safe_agent_environment(&mut command);
        match command
            .arg("-c")
            .arg(&verify_cmd)
            .current_dir(format!("/tmp/wc-{job_id}"))
            .envs(secret_environment)
            .output()
            .await
        {
            Ok(out) => {
                let vrc = python_returncode(out.status);
                if vrc != 0 {
                    ret = 1000 + vrc;
                    verify_err = captured_head(&out.stderr, &out.stdout, 500);
                    log_fn(&format!(
                        "verify_command failed for {job_id}: rc={vrc} err={}",
                        head_chars(&verify_err, 120)
                    ));
                }
            }
            Err(exc) => {
                ret = 1999;
                verify_err = format!("verify_command raised: {exc}");
                log_fn(&format!("verify_command exception for {job_id}: {exc}"));
            }
        }
    }
    // Close the log file BEFORE uploading. Earlier this was deferred
    // until the bottom of the branch, after upload_output ran — so
    // buffered writes from the subprocess weren't flushed to disk
    // when the upload captured the file, producing empty/truncated
    // command_output.log uploads. Confirmed live on 2026-05-06: 3
    // gpt-oss-20b "completions" had zero-byte logs despite the
    // subprocess running.
    slot.close_log();
    let status = if ret == 0 {
        "COMPLETED".to_string()
    } else {
        format!("FAILED exit={ret}")
    };
    write_status(store, &job_id, &status).await?;
    let mut job = slot.slot.job.clone();
    job.state = if ret == 0 {
        job_state::COMPLETED.to_string()
    } else {
        job_state::FAILED.to_string()
    };
    let output_dir = format!("/tmp/wc-{job_id}/output");
    let log_path = format!("{output_dir}/command_output.log");
    let ts = isoformat_utc(Utc::now());
    if ret == 0 {
        job.completed_at = Some(ts);
    } else {
        job.failed_at = Some(ts);
        let tail = tail_log(Path::new(&log_path), 4096);
        job.error = Some(if !verify_err.is_empty() {
            verify_err
        } else if !tail.is_empty() {
            tail
        } else {
            format!("exit={ret} (no stdout/stderr captured)")
        });
    }
    // Durable state transition FIRST so a subsequent upload failure
    // doesn't leave the slot orphaned in running/. upload_output
    // raises on real backend errors; a missing output_dir is a normal
    // happy-path case (job wrote nothing).
    job.peak_vram_gb = job.peak_vram_gb.max(slot.slot.peak_vram_gb);
    // Stamp the per-GPU-probe marker: this agent is 0.4.241+,
    // so smi_job_used_gb measured the MAX single-GPU footprint
    // (grouped by gpu_uuid), not a cross-GPU sum. observed_vram_gb
    // trusts only flagged peaks, so legacy summed records can no
    // longer poison the model max().
    job.peak_vram_per_gpu = true;
    if job.state == job_state::FAILED {
        let error_text = job.error.clone().unwrap_or_default();
        if sizing.escalate_on_oom(store, &mut job, &error_text).await? {
            log_fn(&format!(
                "Job {job_id} OOM-escalated to gpu_mem_gb={}; requeued",
                job.gpu_mem_gb
            ));
            return Ok(SlotOutcome::Done);
        }
    }
    let to_prefix = job.state.clone();
    store.move_job(&job, "running", &to_prefix).await?;
    if Path::new(&output_dir).exists() {
        upload_output(store, &job_id, Path::new(&output_dir)).await?;
    }
    // Mirror to job.output_uri if set. Runs for both COMPLETED and
    // FAILED so debugging logs and partial artifacts also land at
    // the caller's project URI. Failure here is logged, not raised
    // — canonical status/<id>/output/ is already written.
    mirror_to_output_uri(store, &job, log_fn).await;
    // log_file already flushed+closed above before upload_output.
    if job.state == job_state::FAILED {
        log_fn(&format!(
            "Job {job_id} failed ret={ret} error_tail={}",
            tail_chars(job.error.as_deref().unwrap_or(""), 500)
        ));
    } else {
        log_fn(&format!("Job {job_id} {}", job.state));
    }
    Ok(SlotOutcome::Done)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::queue::local_file::LocalBackend;
    use crate::sizing::Sizing;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Arc;

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    /// Unique job id per test case: the workdir is the real /tmp/wc-<id>,
    /// so parallel tests must never collide.
    fn jid(tag: &str) -> String {
        format!(
            "slt{}-{}-{}",
            std::process::id(),
            tag,
            COUNTER.fetch_add(1, Ordering::SeqCst)
        )
    }

    fn store() -> (tempfile::TempDir, JobStorage) {
        let dir = tempfile::tempdir().unwrap();
        let backend = LocalBackend::new(dir.path().to_str().unwrap()).unwrap();
        (dir, JobStorage::with_backend(Arc::new(backend), "local"))
    }

    fn cleanup_workdir(job_id: &str) {
        let _ = std::fs::remove_dir_all(format!("/tmp/wc-{job_id}"));
    }

    /// Advance a slot until it reports Done (or fail the test after ~10s).
    async fn advance_to_done(
        mut slot: ActiveSlot,
        store: &JobStorage,
        sizing: &Sizing,
        log_fn: &mut dyn FnMut(&str),
    ) {
        for _ in 0..100 {
            match advance_slot(slot, store, sizing, false, log_fn)
                .await
                .unwrap()
            {
                SlotOutcome::Running(s) => {
                    slot = s;
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
                SlotOutcome::Done => return,
            }
        }
        panic!("slot did not finish within 10s");
    }

    #[test]
    fn canonical_json_sorts_keys_recursively_and_escapes_non_ascii() {
        let value: Value =
            serde_json::from_str(r#"{"b":1,"a":{"d":[3,{"f":0,"e":2}],"c":"ż"}}"#).unwrap();
        assert_eq!(
            canonical_json(&value),
            r#"{"a":{"c":"\u017c","d":[3,{"e":2,"f":0}]},"b":1}"#
        );
        assert_eq!(canonical_json(&Value::Object(Default::default())), "{}");
    }

    #[test]
    fn tail_log_returns_last_bytes_trimmed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("log");
        assert_eq!(tail_log(&path, 4096), "");
        std::fs::write(&path, "x".repeat(5000) + "tail\n").unwrap();
        let tail = tail_log(&path, 4096);
        assert_eq!(tail.len(), 4095); // 4096 bytes minus the trailing newline
        assert!(tail.ends_with("tail"));
        assert_eq!(tail_log(&path, 10), "xxxxxtail");
    }

    #[test]
    fn python_returncode_maps_exit_codes_and_signals() {
        let status = std::process::Command::new("/bin/sh")
            .args(["-c", "exit 3"])
            .status()
            .unwrap();
        assert_eq!(python_returncode(status), 3);
        let status = std::process::Command::new("/bin/sh")
            .args(["-c", "kill -TERM $$"])
            .status()
            .unwrap();
        assert_eq!(python_returncode(status), -15);
    }

    #[test]
    fn raw_active_disk_refusal_ignores_non_activation_commands() {
        assert_eq!(raw_active_disk_refusal("python -m train --model x"), "");
    }

    #[tokio::test]
    async fn start_slot_drops_duplicate_queued_job() {
        let (_dir, store) = store();
        let id = jid("dup");
        let job = Job::new(&id, "echo hi");
        store.write_job("queue", &job).await.unwrap();
        store.write_job("completed", &job).await.unwrap();
        let mut lines: Vec<String> = Vec::new();
        let mut log = |m: &str| lines.push(m.to_string());
        let res = start_slot(&store, job, "testhost", &mut log, "local")
            .await
            .unwrap();
        assert!(res.is_none());
        assert!(store.read_job("queue", &id).await.unwrap().is_none());
        assert_eq!(
            lines,
            vec![format!("drop duplicate queued {id}: already in completed/")]
        );
    }

    #[tokio::test]
    async fn start_slot_refuses_deprecated_activation_command() {
        let (_dir, store) = store();
        let id = jid("dep");
        let job = Job::new(
            &id,
            "python -m wisent.scripts.activations.extract_and_upload --x",
        );
        store.write_job("queue", &job).await.unwrap();
        let mut lines: Vec<String> = Vec::new();
        let mut log = |m: &str| lines.push(m.to_string());
        let res = start_slot(&store, job, "testhost", &mut log, "local")
            .await
            .unwrap();
        assert!(res.is_none());
        let failed = store.read_job("failed", &id).await.unwrap().unwrap();
        assert_eq!(failed.state, "failed");
        assert!(failed.failed_at.is_some());
        assert!(failed
            .error
            .as_deref()
            .unwrap_or("")
            .starts_with("refusing deprecated"));
        assert!(store.read_job("queue", &id).await.unwrap().is_none());
        assert!(
            lines[0].starts_with(&format!("refuse {id}: refusing deprecated")),
            "{lines:?}"
        );
    }

    #[tokio::test]
    async fn start_slot_refuses_apt_packages_on_local_kind() {
        let (_dir, store) = store();
        let id = jid("apt");
        let mut job = Job::new(&id, "echo hi");
        job.apt_packages = vec!["htop".to_string(), "tmux".to_string()];
        store.write_job("queue", &job).await.unwrap();
        let mut lines: Vec<String> = Vec::new();
        let mut log = |m: &str| lines.push(m.to_string());
        let res = start_slot(&store, job, "testhost", &mut log, "local")
            .await
            .unwrap();
        assert!(res.is_none());
        // The job stays queued for a cloud-kind agent to claim.
        assert!(store.read_job("queue", &id).await.unwrap().is_some());
        assert_eq!(
            lines,
            vec![format!(
                "refuse {id}: apt_packages=['htop', 'tmux'] requested but agent kind=local; \
                 install manually or submit to a cloud-kind agent"
            )]
        );
    }

    #[tokio::test]
    async fn echo_job_completes_end_to_end() {
        let (_dir, store) = store();
        let sizing = Sizing::new();
        let id = jid("ok");
        let job = Job::new(&id, "echo ok");
        store.write_job("queue", &job).await.unwrap();
        let mut lines: Vec<String> = Vec::new();
        let mut log = |m: &str| lines.push(m.to_string());

        let slot = start_slot(&store, job, "testhost", &mut log, "local")
            .await
            .unwrap()
            .expect("slot");
        // RUNNING status + first heartbeat stamped at start; queue -> running.
        let status = store
            .download_text(&format!("status/{id}/status"))
            .await
            .unwrap()
            .unwrap();
        assert!(status.starts_with("RUNNING "), "{status}");
        let hb = store
            .download_text(&format!("status/{id}/heartbeat"))
            .await
            .unwrap()
            .unwrap();
        assert!(hb.starts_with("RUNNING "), "{hb}");
        let running = store.read_job("running", &id).await.unwrap().unwrap();
        assert_eq!(running.state, "running");
        assert_eq!(running.instance_ref.as_deref(), Some("local@testhost"));
        assert!(running.started_at.is_some());
        assert!(store.read_job("queue", &id).await.unwrap().is_none());

        advance_to_done(slot, &store, &sizing, &mut log).await;

        let done = store.read_job("completed", &id).await.unwrap().unwrap();
        assert_eq!(done.state, "completed");
        assert!(done.completed_at.is_some());
        assert!(done.peak_vram_per_gpu);
        assert!(store.read_job("running", &id).await.unwrap().is_none());
        assert_eq!(
            store.read_status(&id).await.unwrap().as_deref(),
            Some("COMPLETED")
        );
        // The log upload contains the job's stdout; the local workdir copy too.
        let uploaded = store
            .download_text(&format!("status/{id}/output/command_output.log"))
            .await
            .unwrap()
            .unwrap();
        assert!(uploaded.contains("ok"), "{uploaded:?}");
        let local =
            std::fs::read_to_string(format!("/tmp/wc-{id}/output/command_output.log")).unwrap();
        assert!(local.contains("ok"), "{local:?}");
        // artifacts.json landed next to the command.
        assert_eq!(
            std::fs::read_to_string(format!("/tmp/wc-{id}/artifacts.json")).unwrap(),
            "{}"
        );
        assert!(
            lines
                .iter()
                .any(|l| l.starts_with(&format!("Started job {id}:"))),
            "{lines:?}"
        );
        assert!(lines.contains(&format!("Job {id} completed")), "{lines:?}");
        cleanup_workdir(&id);
    }

    #[tokio::test]
    async fn exit_code_maps_to_failed_with_exit_status_blob() {
        let (_dir, store) = store();
        let sizing = Sizing::new();
        let id = jid("fail");
        let job = Job::new(&id, "exit 3");
        store.write_job("queue", &job).await.unwrap();
        let mut lines: Vec<String> = Vec::new();
        let mut log = |m: &str| lines.push(m.to_string());

        let slot = start_slot(&store, job, "testhost", &mut log, "local")
            .await
            .unwrap()
            .expect("slot");
        advance_to_done(slot, &store, &sizing, &mut log).await;

        let failed = store.read_job("failed", &id).await.unwrap().unwrap();
        assert_eq!(failed.state, "failed");
        assert!(failed.failed_at.is_some());
        // No stdout/stderr captured -> the exit-code fallback error text.
        assert_eq!(
            failed.error.as_deref(),
            Some("exit=3 (no stdout/stderr captured)")
        );
        let status = store
            .download_text(&format!("status/{id}/status"))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(status, "FAILED exit=3");
        assert!(
            lines
                .iter()
                .any(|l| l.starts_with(&format!("Job {id} failed ret=3"))),
            "{lines:?}"
        );
        cleanup_workdir(&id);
    }

    #[tokio::test]
    async fn failing_verify_command_reverses_completed_to_failed() {
        let (_dir, store) = store();
        let sizing = Sizing::new();
        let id = jid("verify");
        let mut job = Job::new(&id, "exit 0");
        job.verify_command = "exit 7".to_string();
        store.write_job("queue", &job).await.unwrap();
        let mut lines: Vec<String> = Vec::new();
        let mut log = |m: &str| lines.push(m.to_string());

        let slot = start_slot(&store, job, "testhost", &mut log, "local")
            .await
            .unwrap()
            .expect("slot");
        advance_to_done(slot, &store, &sizing, &mut log).await;

        // Verify rc=7 reverses the exit-0 completion into FAILED exit=1007.
        assert!(store.read_job("completed", &id).await.unwrap().is_none());
        let failed = store.read_job("failed", &id).await.unwrap().unwrap();
        assert_eq!(failed.state, "failed");
        assert_eq!(
            failed.error.as_deref(),
            Some("exit=1007 (no stdout/stderr captured)")
        );
        let status = store
            .download_text(&format!("status/{id}/status"))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(status, "FAILED exit=1007");
        assert!(
            lines
                .iter()
                .any(|l| l.starts_with(&format!("verify_command failed for {id}: rc=7"))),
            "{lines:?}"
        );
        cleanup_workdir(&id);
    }

    #[tokio::test]
    async fn request_yield_grace_path_requeues_after_hook_exits_child() {
        let (_dir, store) = store();
        let id = jid("grace");
        let mut job = Job::new(&id, "sleep 300");
        // The save-and-sync hook: persist a marker, then signal the
        // process-group leader so the job exits inside the grace window.
        job.yield_command = format!("touch /tmp/wc-{id}/yield.marker && kill \"$WC_JOB_PID\"");
        job.yield_grace_seconds = 30;
        job.yieldable = true;
        store.write_job("queue", &job).await.unwrap();
        let mut lines: Vec<String> = Vec::new();
        let mut log = |m: &str| lines.push(m.to_string());

        let slot = start_slot(&store, job, "testhost", &mut log, "local")
            .await
            .unwrap()
            .expect("slot");
        let pid = slot.pid();
        assert!(request_yield(slot, &store, &mut log).await.unwrap());

        // The hook ran in the job workdir with WC_JOB_PID set.
        assert!(Path::new(&format!("/tmp/wc-{id}/yield.marker")).exists());
        // Requeued: running -> queue, QUEUED, yield_count++, refs cleared.
        assert!(store.read_job("running", &id).await.unwrap().is_none());
        let queued = store.read_job("queue", &id).await.unwrap().unwrap();
        assert_eq!(queued.state, "queued");
        assert_eq!(queued.yield_count, 1);
        assert!(queued.instance_ref.is_none());
        assert!(queued.started_at.is_none());
        let status = store
            .download_text(&format!("status/{id}/status"))
            .await
            .unwrap()
            .unwrap();
        assert!(status.starts_with("YIELDED "), "{status}");
        // The output upload still ran on the yield path.
        assert!(store
            .download_text(&format!("status/{id}/output/command_output.log"))
            .await
            .unwrap()
            .is_some());
        assert!(
            lines
                .iter()
                .any(|l| l.starts_with(&format!("yield: requesting yield of {id} (grace=30s"))),
            "{lines:?}"
        );
        assert!(
            lines.contains(&format!("yield: {id} requeued (yield_count=1)")),
            "{lines:?}"
        );
        assert!(!lines.iter().any(|l| l.contains("SIGKILL")), "{lines:?}");
        // Defensive: nothing in the process group survives the test.
        let _ = nix::sys::signal::killpg(Pid::from_raw(pid), Signal::SIGKILL);
        cleanup_workdir(&id);
    }

    #[tokio::test]
    async fn request_yield_sigkills_group_when_grace_is_blown() {
        let (_dir, store) = store();
        let id = jid("kill");
        let mut job = Job::new(&id, "sleep 300");
        job.yield_grace_seconds = 2; // no hook: the child ignores nothing, the grace just expires
        job.yieldable = true;
        store.write_job("queue", &job).await.unwrap();
        let mut lines: Vec<String> = Vec::new();
        let mut log = |m: &str| lines.push(m.to_string());

        let slot = start_slot(&store, job, "testhost", &mut log, "local")
            .await
            .unwrap()
            .expect("slot");
        let pid = slot.pid();
        let started = Instant::now();
        assert!(request_yield(slot, &store, &mut log).await.unwrap());
        let elapsed = started.elapsed();

        // ~2s grace, not the default 120s; the group leader is dead.
        assert!(elapsed < Duration::from_secs(15), "{elapsed:?}");
        assert!(!helpers::pid_alive(pid));
        let queued = store.read_job("queue", &id).await.unwrap().unwrap();
        assert_eq!(queued.state, "queued");
        assert_eq!(queued.yield_count, 1);
        assert!(
            lines
                .iter()
                .any(|l| l.contains(&format!("still alive after grace — SIGKILL group {pid}"))),
            "{lines:?}"
        );
        cleanup_workdir(&id);
    }
}
