//! Host-side blue-green release reconciliation.
//!
//! The agent consumes exact desired release coordinates from the canonical
//! registry, verifies signed immutable artifacts, stages them in versioned
//! directories, starts a candidate on an internal port, and atomically changes
//! a stable loopback proxy. The previous process stays warm for the rollback
//! window. Failed digests are quarantined and cannot loop indefinitely.

use std::collections::BTreeMap;
use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::net::SocketAddr;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use chrono::{DateTime, Utc};
use nix::sys::signal::{kill, Signal};
use nix::sys::wait::{waitpid, WaitPidFlag, WaitStatus};
use nix::unistd::Pid;
use serde::{Deserialize, Serialize};
use tokio::io::copy_bidirectional;
use tokio::net::{TcpListener, TcpStream};

use crate::release_cause::{self, QuarantineCause};
use crate::release_control::{
    self, BlueGreenServing, DesiredRelease, ProductReleasePolicy, QualificationStatus,
    ReleaseArtifactRef, ReleaseControl, ReleaseManifest, ReleaseTargetPolicy, StrategyKind,
};

const STATE_SCHEMA: u32 = 1;
const STATUS_SCHEMA: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RolloutPhase {
    Idle,
    Downloaded,
    Verified,
    Staged,
    CandidateRunning,
    Ready,
    Routed,
    Monitoring,
    Committed,
    RolledBack,
    Failed,
    Quarantined,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProcessRecord {
    pub version: String,
    pub artifact_sha256: String,
    pub manifest_sha256: String,
    pub port: u16,
    pub pid: i32,
    pub release_dir: String,
    pub started_at: DateTime<Utc>,
}

/// One digest this host refuses to roll out again, why, and what that reason
/// actually means.
///
/// `reason` is the sentence the agent composed at the moment it gave up, and it
/// leads with what the agent saw from outside the candidate. `cause` is the
/// name derived from it and from the candidate's own log, and `evidence` is the
/// one line that name was read from. All three are kept: a record that stored
/// only the cause could not be re-read when the vocabulary grows, and a record
/// that stored only the reason is what left twenty rows of truncated stderr
/// unreadable for a month.
///
/// `cause` and `evidence` default, because every record already on the fleet
/// was written without them and this struct refuses unknown fields — a missing
/// name has to read as [`QuarantineCause::Unclassified`], not as a parse
/// failure that would strand the rollout.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QuarantineRecord {
    pub reason: String,
    pub quarantined_at: DateTime<Utc>,
    #[serde(default)]
    pub cause: QuarantineCause,
    #[serde(default)]
    pub evidence: String,
}

impl QuarantineRecord {
    /// Record one refusal, naming its cause from the reason itself.
    ///
    /// For the sites that never read a candidate log — a rollback, a rejected
    /// rollback-compatibility declaration, a fetch that failed — the reason is
    /// the whole of the available evidence.
    fn new(reason: String) -> Self {
        let classified = release_cause::classify(&reason);
        Self {
            reason,
            quarantined_at: Utc::now(),
            cause: classified.cause,
            evidence: classified.evidence,
        }
    }

    /// Record one refusal whose cause was read from more of the candidate's
    /// own output than the reason could carry.
    fn classified(reason: String, classified: release_cause::Classification) -> Self {
        Self {
            reason,
            quarantined_at: Utc::now(),
            cause: classified.cause,
            evidence: classified.evidence,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HostReleaseState {
    pub schema_version: u32,
    pub product: String,
    pub target: String,
    pub rollout_generation: u64,
    pub phase: RolloutPhase,
    #[serde(default)]
    pub active: Option<ProcessRecord>,
    #[serde(default)]
    pub previous: Option<ProcessRecord>,
    #[serde(default)]
    pub candidate: Option<ProcessRecord>,
    #[serde(default)]
    pub proxy_pid: Option<i32>,
    #[serde(default)]
    pub cutover_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub quarantined: BTreeMap<String, QuarantineRecord>,
    #[serde(default)]
    pub detail: String,
    pub updated_at: DateTime<Utc>,
}

impl HostReleaseState {
    fn new(product: &str, target: &str) -> Self {
        Self {
            schema_version: STATE_SCHEMA,
            product: product.to_string(),
            target: target.to_string(),
            rollout_generation: 0,
            phase: RolloutPhase::Idle,
            active: None,
            previous: None,
            candidate: None,
            proxy_pid: None,
            cutover_at: None,
            quarantined: BTreeMap::new(),
            detail: String::new(),
            updated_at: Utc::now(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProxyState {
    generation: u64,
    upstream: String,
    updated_at: DateTime<Utc>,
}

#[derive(Debug, Serialize)]
struct PublishedStatus<'a> {
    schema_version: u32,
    product: &'a str,
    target: &'a str,
    rollout_generation: u64,
    phase: RolloutPhase,
    active_version: Option<&'a str>,
    active_sha256: Option<&'a str>,
    previous_version: Option<&'a str>,
    detail: &'a str,
    updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub(crate) struct ActiveBinary {
    pub path: PathBuf,
    pub version: String,
    pub platform: String,
    pub artifact_sha256: String,
    pub manifest_sha256: String,
}

/// The rollout state document one product keeps on one host.
///
/// Spelled once because `stado release quarantine` names this file over the
/// registry SSH channel while the agent opens it locally, and a command that
/// reads `<product>.json` from its own second spelling is a command that reads
/// a file no agent writes.
pub fn host_state_path(state_dir: &str, product: &str) -> String {
    format!("{state_dir}/{product}.json")
}

fn state_path(target: &ReleaseTargetPolicy, product: &str) -> PathBuf {
    PathBuf::from(host_state_path(&target.state_dir, product))
}

fn proxy_state_path(target: &ReleaseTargetPolicy, product: &str) -> PathBuf {
    Path::new(&target.state_dir).join(format!("{product}-proxy.json"))
}

/// A non-blocking exclusive advisory lock on one file in a release state
/// directory.
///
/// Crate-visible with a caller-supplied stem because the unit-image revisit
/// pass needs one lock per HOST over its whole observe -> restart -> record
/// sequence, and this is already the shape of that lock: same `fs2` advisory
/// mode, same `O_NOFOLLOW`, same `WouldBlock` means another holder rather than
/// a failure. A second implementation beside it would be a second answer to
/// "is somebody already doing this".
pub(crate) struct StateLock {
    file: File,
}

impl Drop for StateLock {
    fn drop(&mut self) {
        let _ = fs2::FileExt::unlock(&self.file);
    }
}

/// `Ok(None)` when another process holds it. The caller decides what a busy
/// lock means; nothing here waits.
pub(crate) fn acquire_state_lock(state_dir: &str, stem: &str) -> Result<Option<StateLock>, String> {
    let state_dir = Path::new(state_dir);
    std::fs::create_dir_all(state_dir).map_err(|error| {
        format!(
            "cannot create release state directory {}: {error}",
            state_dir.display()
        )
    })?;
    let path = state_dir.join(format!("{stem}.lock"));
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .custom_flags(nix::libc::O_NOFOLLOW)
        .mode(0o600)
        .open(&path)
        .map_err(|error| format!("cannot open release lock {}: {error}", path.display()))?;
    if !file
        .metadata()
        .map_err(|error| format!("cannot inspect release lock {}: {error}", path.display()))?
        .is_file()
    {
        return Err(format!(
            "release lock is not a regular file: {}",
            path.display()
        ));
    }
    match fs2::FileExt::try_lock_exclusive(&file) {
        Ok(()) => Ok(Some(StateLock { file })),
        Err(error)
            if error.kind() == io::ErrorKind::WouldBlock
                || matches!(
                    error.raw_os_error(),
                    Some(code)
                        if code == nix::libc::EACCES || code == nix::libc::EAGAIN
                ) =>
        {
            Ok(None)
        }
        Err(error) => Err(format!(
            "cannot acquire release lock {}: {error}",
            path.display()
        )),
    }
}

fn acquire_product_reconcile_lock(
    target: &ReleaseTargetPolicy,
    product: &str,
) -> Result<Option<StateLock>, String> {
    acquire_state_lock(&target.state_dir, &format!("{product}.reconcile"))
}

/// The exact bytes one document is committed as: compact JSON and one trailing
/// newline.
fn document_bytes<T: Serialize>(value: &T) -> Result<Vec<u8>, String> {
    let mut bytes = serde_json::to_vec(value)
        .map_err(|error| format!("cannot encode release state: {error}"))?;
    bytes.push(b'\n');
    Ok(bytes)
}

/// The bytes [`atomic_json`] would commit for one rollout state document.
///
/// `stado release quarantine clear` rewrites this file from off-host, and what
/// it sends has to be what this agent would have written: the digest the host
/// is asked to verify after the write is taken over exactly these bytes, so an
/// encoding that drifted from the agent's own fails that check instead of
/// quietly leaving two shapes of the same document in the fleet.
pub fn state_document_bytes(state: &HostReleaseState) -> Result<Vec<u8>, String> {
    document_bytes(state)
}

pub(crate) fn atomic_json<T: Serialize>(path: &Path, value: &T) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| format!("state path has no parent: {}", path.display()))?;
    std::fs::create_dir_all(parent).map_err(|error| {
        format!(
            "cannot create state directory {}: {error}",
            parent.display()
        )
    })?;
    let staging = parent.join(format!(".state-{}", uuid::Uuid::new_v4().simple()));
    let bytes = document_bytes(value)?;
    {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&staging)
            .map_err(|error| {
                format!("cannot create state staging {}: {error}", staging.display())
            })?;
        file.write_all(&bytes)
            .and_then(|()| file.sync_all())
            .map_err(|error| {
                format!("cannot write state staging {}: {error}", staging.display())
            })?;
    }
    std::fs::rename(&staging, path)
        .map_err(|error| format!("cannot commit release state {}: {error}", path.display()))
}

/// Decode one rollout state document and refuse it unless it is the one that
/// was asked for.
///
/// Lifted out of [`load_state`] because `stado release quarantine` reads the
/// same document back over the registry SSH channel and then rewrites it.
/// These three checks are all that stands between a mistyped host name and a
/// state file overwritten with another host's rollout, so both readers make
/// them from one place rather than from two copies that agree today. `origin`
/// is whatever the caller can show an operator: a local path, or the remote
/// path it read.
pub fn parse_state_document(
    payload: &[u8],
    product: &str,
    target_name: &str,
    origin: &str,
) -> Result<HostReleaseState, String> {
    let state: HostReleaseState = serde_json::from_slice(payload)
        .map_err(|error| format!("invalid release state {origin}: {error}"))?;
    if state.schema_version != STATE_SCHEMA
        || state.product != product
        || state.target != target_name
    {
        return Err(format!("release state identity mismatch at {origin}"));
    }
    Ok(state)
}

fn load_state(
    target: &ReleaseTargetPolicy,
    product: &str,
    target_name: &str,
) -> Result<HostReleaseState, String> {
    let path = state_path(target, product);
    if !path.exists() {
        return Ok(HostReleaseState::new(product, target_name));
    }
    let bytes = std::fs::read(&path)
        .map_err(|error| format!("cannot read release state {}: {error}", path.display()))?;
    parse_state_document(&bytes, product, target_name, &path.display().to_string())
}

fn save_state(target: &ReleaseTargetPolicy, state: &mut HostReleaseState) -> Result<(), String> {
    state.updated_at = Utc::now();
    atomic_json(&state_path(target, &state.product), state)
}

fn pid_alive(pid: i32) -> bool {
    if pid <= 1 {
        return false;
    }
    let pid = Pid::from_raw(pid);
    match waitpid(pid, Some(WaitPidFlag::WNOHANG)) {
        Ok(WaitStatus::StillAlive) => true,
        Ok(WaitStatus::Exited(..) | WaitStatus::Signaled(..)) => false,
        Ok(_) => true,
        Err(nix::errno::Errno::ECHILD) => kill(pid, None).is_ok(),
        Err(nix::errno::Errno::ESRCH) => false,
        Err(_) => kill(pid, None).is_ok(),
    }
}

fn terminate(record: &ProcessRecord) {
    if pid_alive(record.pid) {
        let group = Pid::from_raw(-record.pid);
        if kill(group, Signal::SIGTERM).is_err() {
            let _ = kill(Pid::from_raw(record.pid), Signal::SIGTERM);
        }
    }
}

/// One release's own stdout or stderr on its host.
///
/// A brama candidate died inside ninety seconds and the rollout record said
/// only `candidate did not become ready within 90s: pid 46748 is gone`, while
/// the candidate's account of itself sat unread in
/// `<logs_root>/brama-0.2.27.err`. The name is public so an operator-facing
/// command reads the file the agent actually wrote rather than a second guess
/// at this format.
pub fn host_log_path(logs_root: &str, product: &str, version: &str, stream: &str) -> String {
    format!("{logs_root}/{product}-{version}.{stream}")
}

fn release_log_path(
    target: &ReleaseTargetPolicy,
    product: &str,
    version: &str,
    stream: &str,
) -> PathBuf {
    PathBuf::from(host_log_path(&target.logs_root, product, version, stream))
}

fn release_log(
    target: &ReleaseTargetPolicy,
    product: &str,
    version: &str,
    stream: &str,
) -> Result<File, String> {
    std::fs::create_dir_all(&target.logs_root)
        .map_err(|error| format!("cannot create release logs {}: {error}", target.logs_root))?;
    let path = release_log_path(target, product, version, stream);
    OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .map_err(|error| format!("cannot open release log {}: {error}", path.display()))
}

/// One release log as a failure record uses it.
///
/// A quarantine reason used to say only what the agent observed from outside --
/// "pid is gone", "refused the connection" -- and the product's own account of
/// why it exited sat in a file nobody had opened. Two days of one session went
/// into reading those files by hand, one candidate at a time, so the reason now
/// carries the tail with it. Missing or unreadable is reported, never silently
/// dropped: a reason that mentions no log at all would send the next reader
/// hunting for one.
struct LogEvidence {
    /// `<path>: <tail>` for the reason string, or a bracketed note when there
    /// is nothing to quote.
    rendered: String,
    /// Every byte the product wrote, for the classifier. Empty exactly when
    /// the file was missing, unreadable or empty.
    body: String,
}

/// Quote a bounded window of `text` that keeps both of its ends.
///
/// This used to keep the first `max_chars` characters and drop everything
/// after them, and eight of the twenty live `brama` records were clipped that
/// way. Measured against those records the head-clip did not actually lose a
/// cause: every decisive sentence present sits in the first 57% of its tail,
/// the latest being `brama-0.2.55`'s `no value at ...#value`. So this is not a
/// fix for an observed misclassification, and the fix for that is elsewhere --
/// the cause is now derived from the whole log rather than from this window.
///
/// It is still the wrong end to drop. Nothing holds a decisive line near the
/// head: 57% of a 1200-character budget is already past the midpoint, and the
/// end of a dying process's log is exactly where a panic, an abort message or a
/// final error lands. Keeping both ends costs the same width and cannot lose
/// either. The elision carries the count of what went rather than a bare
/// ellipsis, because a reader who cannot see how much was dropped cannot tell a
/// trimmed line from a whole one.
fn clip_middle(text: &str, max_chars: usize) -> String {
    let total = text.chars().count();
    if total <= max_chars {
        return text.to_string();
    }
    let head_chars = max_chars / 2;
    let tail_chars = max_chars - head_chars;
    let head_end = text
        .char_indices()
        .nth(head_chars)
        .map_or(text.len(), |(index, _)| index);
    let tail_start = text
        .char_indices()
        .nth(total - tail_chars)
        .map_or(text.len(), |(index, _)| index);
    format!(
        "{} …{} elided… {}",
        &text[..head_end],
        total - max_chars,
        &text[tail_start..]
    )
}

/// Read one release log once, for both the reason and the classifier.
///
/// Read once rather than twice on purpose: the reason quotes a bounded tail and
/// the classifier wants every byte, and opening the file a second time would
/// let the two disagree about what the product said.
fn log_evidence(path: &Path, lines: usize, max_chars: usize) -> LogEvidence {
    let note = |text: String| LogEvidence {
        rendered: text,
        body: String::new(),
    };
    let body = match std::fs::read_to_string(path) {
        Ok(body) => body,
        Err(error) => return note(format!("[{} unreadable: {error}]", path.display())),
    };
    let trimmed = body.trim_end();
    if trimmed.is_empty() {
        return note(format!("[{} is empty]", path.display()));
    }
    let mut kept: Vec<&str> = trimmed.lines().rev().take(lines).collect();
    kept.reverse();
    let clipped = clip_middle(&kept.join(" | "), max_chars);
    LogEvidence {
        rendered: format!("{}: {clipped}", path.display()),
        body,
    }
}

fn expand_home(value: &str, home: &str) -> String {
    value.replace("{home}", home)
}

fn spawn_release(
    product: &str,
    policy: &ProductReleasePolicy,
    target: &ReleaseTargetPolicy,
    manifest: &ReleaseManifest,
    release_dir: &Path,
    port: u16,
) -> Result<ProcessRecord, String> {
    let launcher = release_dir.join(&policy.launcher);
    let binary = release_dir.join(&policy.binary);
    for path in [&launcher, &binary] {
        let metadata = std::fs::symlink_metadata(path)
            .map_err(|error| format!("release entry {} is unavailable: {error}", path.display()))?;
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            return Err(format!(
                "release entry is not a regular file: {}",
                path.display()
            ));
        }
    }
    let runtime = Path::new(&target.runtime_root)
        .join(product)
        .join(format!("{}-{port}", manifest.version));
    std::fs::create_dir_all(&runtime).map_err(|error| {
        format!(
            "cannot create candidate runtime {}: {error}",
            runtime.display()
        )
    })?;
    let stdout = release_log(target, product, &manifest.version, "out")?;
    let stderr = release_log(target, product, &manifest.version, "err")?;
    let mut command = Command::new("/usr/bin/sudo");
    command
        .args(["-n", "-u", &target.run_as_user, "-H", "/usr/bin/env"])
        .arg(format!("HOME={}", target.home))
        // `sudo` replaces PATH with its own `secure_path`, which carries no
        // Homebrew prefix, and a candidate started with that PATH cannot find
        // the helpers its product shells out to. `ask_wall` already sets this
        // exact list for the same reason -- "the decrypt helper lives there,
        // and without it every answer would be an unreachable one" -- and the
        // managed launchd units this rollout replaces carry it too, so a
        // candidate without it is the only shape of the process that has ever
        // run without a PATH.
        //
        // It cost this fleet three days. Every skarbiec candidate from
        // 2026-09-01 onward failed readiness with `stored item cannot be
        // decrypted: spawn gpg: No such file or directory`, was quarantined,
        // and left the vault unreadable; the object plane's verifiers read
        // Skarbiec, so the whole control plane answered `503 object
        // authorization unavailable`, and every Brama agent identity 401'd
        // behind it. The launchd unit had the PATH, the rollout did not, and
        // nothing compared the two declarations.
        //
        // `policy.environment` is applied after this, so a product that needs
        // a different PATH still declares one and wins.
        .arg("PATH=/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin")
        .arg(format!("STADO_RELEASE_PRODUCT={product}"))
        .arg(format!("STADO_RELEASE_VERSION={}", manifest.version))
        .arg(format!("STADO_RELEASE_PLATFORM={}", manifest.platform))
        .arg(format!("STADO_RELEASE_SHA256={}", manifest.artifact_sha256))
        .arg(format!("{}={}", policy.binary_env, binary.display()))
        .arg(format!("{}={port}", policy.port_env))
        .arg(format!("{}={}", policy.runtime_env, runtime.display()));
    for (name, value) in &policy.environment {
        command.arg(format!("{name}={}", expand_home(value, &target.home)));
    }
    let child = command
        .arg(&launcher)
        .process_group(0)
        .stdin(Stdio::null())
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr))
        .spawn()
        .map_err(|error| {
            format!(
                "cannot start {product} {} candidate: {error}",
                manifest.version
            )
        })?;
    Ok(ProcessRecord {
        version: manifest.version.clone(),
        artifact_sha256: manifest.artifact_sha256.clone(),
        manifest_sha256: release_control::sha256_bytes(&release_control::canonical_manifest(
            manifest,
        )?),
        port,
        pid: child.id() as i32,
        release_dir: release_dir.display().to_string(),
        started_at: Utc::now(),
    })
}

/// Why a candidate is not ready yet, or `None` when it is.
///
/// This used to be a `bool`, and the quarantine record it fed said only
/// "candidate did not become ready before deadline". A brama candidate was
/// quarantined four times across twelve days with its own log showing
/// `Starting brama server on 127.0.0.1:18080` seconds earlier, and the record
/// could not distinguish a dead process from a refused connection from an HTTP
/// status. Every hypothesis had to be excluded by reading the product's source.
async fn not_ready_because(record: &ProcessRecord, path: &str) -> Option<String> {
    if !pid_alive(record.pid) {
        return Some(format!("pid {} is gone", record.pid));
    }
    let url = format!("http://127.0.0.1:{}{}", record.port, path);
    match reqwest::Client::new()
        .get(&url)
        .timeout(Duration::from_secs(3))
        .send()
        .await
    {
        Ok(response) if response.status().is_success() => None,
        Ok(response) => Some(format!("{url} answered HTTP {}", response.status())),
        Err(error) if error.is_timeout() => Some(format!("{url} did not answer within 3s")),
        Err(error) if error.is_connect() => Some(format!("{url} refused the connection")),
        Err(error) => Some(format!("{url} failed: {error}")),
    }
}

async fn ready(record: &ProcessRecord, path: &str) -> bool {
    not_ready_because(record, path).await.is_none()
}

/// Wait for readiness, returning the last reason it was refused.
async fn await_ready_because(
    record: &ProcessRecord,
    readiness_path: &str,
    seconds: u64,
) -> Option<String> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(seconds);
    loop {
        let reason = match not_ready_because(record, readiness_path).await {
            None => return None,
            Some(reason) => reason,
        };
        if tokio::time::Instant::now() >= deadline {
            return Some(reason);
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}

fn process_executable_matches(pid: i32, expected: &Path) -> bool {
    #[cfg(target_os = "linux")]
    {
        let Ok(actual) = std::fs::read_link(format!("/proc/{pid}/exe")) else {
            return false;
        };
        let actual = actual.to_string_lossy();
        actual.strip_suffix(" (deleted)").unwrap_or(&actual) == expected.to_string_lossy()
    }
    #[cfg(not(target_os = "linux"))]
    {
        let Ok(output) = Command::new("/bin/ps")
            .args(["-p", &pid.to_string(), "-o", "comm="])
            .output()
        else {
            return false;
        };
        output.status.success()
            && String::from_utf8_lossy(&output.stdout).trim() == expected.to_string_lossy()
    }
}

fn proxy_process_matches(
    pid: i32,
    target: &ReleaseTargetPolicy,
    serving: &BlueGreenServing,
    product: &str,
) -> Result<bool, String> {
    if !pid_alive(pid) {
        return Ok(false);
    }
    let executable = std::env::current_exe()
        .map_err(|error| format!("cannot resolve Stado executable: {error}"))?;
    if !process_executable_matches(pid, &executable) {
        return Ok(false);
    }
    let expected_command = format!(
        "{} release proxy --state {} --bind {}",
        executable.display(),
        proxy_state_path(target, product).display(),
        serving.stable_bind
    );
    let output = Command::new("/bin/ps")
        .args(["-ww", "-p", &pid.to_string(), "-o", "command="])
        .output()
        .map_err(|error| format!("cannot inspect stable proxy pid {pid}: {error}"))?;
    Ok(output.status.success()
        && String::from_utf8_lossy(&output.stdout).trim() == expected_command)
}

/// Find the live Stado proxy whose executable and complete argument vector own
/// this exact state file and bind.
///
/// A release proxy binds before entering its accept loop and exits when the
/// bind fails. After the caller's spawn grace period, a live exact match is
/// therefore the process that owns the bind, not merely another program that
/// happens to answer the product's readiness URL.
fn exact_proxy_pid(
    target: &ReleaseTargetPolicy,
    serving: &BlueGreenServing,
    product: &str,
) -> Result<Option<i32>, String> {
    let executable = std::env::current_exe()
        .map_err(|error| format!("cannot resolve Stado executable: {error}"))?;
    let expected_command = format!(
        "{} release proxy --state {} --bind {}",
        executable.display(),
        proxy_state_path(target, product).display(),
        serving.stable_bind
    );
    let output = Command::new("/bin/ps")
        .args(["axww", "-o", "pid=", "-o", "command="])
        .output()
        .map_err(|error| format!("cannot inspect stable proxy processes: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "cannot inspect stable proxy processes: /bin/ps exited {}",
            output.status
        ));
    }
    let mut matches = String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| {
            let line = line.trim_start();
            let split = line.find(char::is_whitespace)?;
            let pid = line[..split].parse::<i32>().ok()?;
            (line[split..].trim_start() == expected_command
                && pid_alive(pid)
                && process_executable_matches(pid, &executable))
            .then_some(pid)
        })
        .collect::<Vec<_>>();
    matches.sort_unstable();
    matches.dedup();
    match matches.as_slice() {
        [pid] => Ok(Some(*pid)),
        [] => Ok(None),
        many => Err(format!(
            "{} live Stado release proxies claim {} with state {}",
            many.len(),
            serving.stable_bind,
            proxy_state_path(target, product).display()
        )),
    }
}

/// Prove that the exact live Stado proxy routes the exact staged release and
/// that the product accepts its declared readiness request on the stable bind.
///
/// Release identity does not belong to the product's readiness document. The
/// signed manifest and immutable archive establish the candidate's version and
/// digest before it starts; the process record and proxy target then bind that
/// identity to one candidate port. Requiring an undeclared `build.version`
/// field here made otherwise-valid readiness contracts impossible to satisfy.
async fn stable_bind_answer(
    proxy_pid: i32,
    target: &ReleaseTargetPolicy,
    serving: &BlueGreenServing,
    product: &str,
    generation: u64,
    active: &ProcessRecord,
) -> Result<(), String> {
    if !pid_alive(proxy_pid) {
        return Err(format!("stable release proxy pid {proxy_pid} is gone"));
    }
    let marker = std::fs::read(marker_path(Path::new(&active.release_dir))).map_err(|error| {
        format!(
            "cannot read active release identity {}: {error}",
            active.release_dir
        )
    })?;
    let manifest: ReleaseManifest = serde_json::from_slice(&marker)
        .map_err(|error| format!("active release identity is invalid: {error}"))?;
    let manifest_sha =
        release_control::sha256_bytes(&release_control::canonical_manifest(&manifest)?);
    if manifest_sha != active.manifest_sha256
        || manifest.version != active.version
        || manifest.artifact_sha256 != active.artifact_sha256
    {
        return Err("active process does not match its immutable release identity".to_string());
    }

    let proxy_path = proxy_state_path(target, product);
    let proxy: ProxyState =
        serde_json::from_slice(&std::fs::read(&proxy_path).map_err(|error| {
            format!("cannot read proxy target {}: {error}", proxy_path.display())
        })?)
        .map_err(|error| format!("invalid proxy target {}: {error}", proxy_path.display()))?;
    let expected_upstream = format!("127.0.0.1:{}", active.port);
    if proxy.generation != generation || proxy.upstream != expected_upstream {
        return Err(format!(
            "stable proxy target is generation {} upstream {}, expected generation {generation} upstream {expected_upstream}",
            proxy.generation, proxy.upstream
        ));
    }

    let url = format!("http://{}{}", serving.stable_bind, serving.readiness_path);
    let response = reqwest::Client::new()
        .get(&url)
        .timeout(Duration::from_secs(3))
        .send()
        .await
        .map_err(|error| {
            if error.is_timeout() {
                format!("{url} did not answer within 3s")
            } else if error.is_connect() {
                format!("{url} refused the connection")
            } else {
                format!("{url} failed: {error}")
            }
        })?;
    if !response.status().is_success() {
        return Err(format!("{url} answered HTTP {}", response.status()));
    }
    Ok(())
}

fn stop_legacy(target: &ReleaseTargetPolicy) -> Result<(), String> {
    let Some(label) = target.legacy_launchd_label.as_deref() else {
        return Ok(());
    };
    let status = Command::new("/usr/bin/sudo")
        .args([
            "-n",
            "/bin/launchctl",
            "bootout",
            &format!("system/{label}"),
        ])
        .status()
        .map_err(|error| format!("cannot disable legacy launchd service {label}: {error}"))?;
    // This asks for a state, not an action: the legacy unit must not hold the
    // port before the proxy binds it. launchd answers 113 ("Could not find
    // specified service") when the label is not loaded, which IS that state,
    // and refusing it stopped the proxy step dead on 2026-09-03 -- both
    // candidates healthy on their candidate ports, nothing serving either
    // stable bind, and the control plane 503 behind that. 3 and 5 were
    // already tolerated for exactly this reason; 113 belongs with them.
    if status.success() || matches!(status.code(), Some(3) | Some(5) | Some(113)) {
        Ok(())
    } else {
        Err(format!(
            "legacy launchd service {label} bootout exited with {status}"
        ))
    }
}

fn restore_legacy(target: &ReleaseTargetPolicy) -> Result<(), String> {
    let Some(plist) = target.legacy_launchd_plist.as_deref() else {
        return Ok(());
    };
    let status = Command::new("/usr/bin/sudo")
        .args(["-n", "/bin/launchctl", "bootstrap", "system", plist])
        .status()
        .map_err(|error| format!("cannot restore legacy launchd service {plist}: {error}"))?;
    if status.success() || status.code() == Some(5) {
        Ok(())
    } else {
        Err(format!(
            "legacy launchd service bootstrap exited with {status}"
        ))
    }
}

fn write_proxy_target(
    target: &ReleaseTargetPolicy,
    product: &str,
    generation: u64,
    port: u16,
) -> Result<(), String> {
    atomic_json(
        &proxy_state_path(target, product),
        &ProxyState {
            generation,
            upstream: format!("127.0.0.1:{port}"),
            updated_at: Utc::now(),
        },
    )
}

fn start_proxy(
    target: &ReleaseTargetPolicy,
    serving: &BlueGreenServing,
    product: &str,
    generation: u64,
    port: u16,
) -> Result<i32, String> {
    write_proxy_target(target, product, generation, port)?;
    let executable = std::env::current_exe()
        .map_err(|error| format!("cannot resolve Stado executable: {error}"))?;
    let stdout = release_log(target, product, "proxy", "out")?;
    let stderr = release_log(target, product, "proxy", "err")?;
    let child = Command::new(executable)
        .args([
            "release",
            "proxy",
            "--state",
            &proxy_state_path(target, product).display().to_string(),
            "--bind",
            &serving.stable_bind,
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr))
        .spawn()
        .map_err(|error| format!("cannot start stable release proxy: {error}"))?;
    Ok(child.id() as i32)
}

async fn ensure_active_proxy(
    target: &ReleaseTargetPolicy,
    serving: &BlueGreenServing,
    product: &str,
    generation: u64,
    active: &ProcessRecord,
    state: &mut HostReleaseState,
) -> Result<(), String> {
    if !ready(active, &serving.readiness_path).await {
        return Err("active release lost readiness".to_string());
    }
    write_proxy_target(target, product, generation, active.port)?;

    let recorded_proxy = match state.proxy_pid {
        Some(proxy_pid) if proxy_process_matches(proxy_pid, target, serving, product)? => {
            Some(proxy_pid)
        }
        _ => None,
    };
    let proxy_pid = if let Some(proxy_pid) = recorded_proxy {
        proxy_pid
    } else if let Some(proxy_pid) = exact_proxy_pid(target, serving, product)? {
        proxy_pid
    } else {
        stop_legacy(target)?;
        let spawned_pid = start_proxy(target, serving, product, generation, active.port)?;
        tokio::time::sleep(Duration::from_millis(200)).await;
        let proxy_pid = exact_proxy_pid(target, serving, product)
            .map_err(|why| format!("stable release proxy failed to start: {why}"))?
            .ok_or_else(|| "spawned stable release proxy is not live".to_string())?;
        if proxy_pid != spawned_pid {
            return Err(format!(
                "spawned stable release proxy pid {spawned_pid}, but exact owner is pid {proxy_pid}"
            ));
        }
        proxy_pid
    };

    state.proxy_pid = Some(proxy_pid);
    stable_bind_answer(proxy_pid, target, serving, product, generation, active)
        .await
        .map_err(|why| format!("stable release proxy is invalid: {why}"))
}

async fn fetch_release_bytes(uri: &str) -> Result<Vec<u8>, String> {
    // The release channel is served publicly over the object API.
    // Without STADO_API_URL, JobStorage::read_bytes on the canonical root
    // prefix serves a stale copy and the agent quarantines the release.
    crate::cli::storage::fetch_object(uri)
        .await
        .map_err(|e| e.to_string())
}

pub(crate) async fn fetch_candidate(
    control: &ReleaseControl,
    product: &str,
    desired: &DesiredRelease,
    artifact: &ReleaseArtifactRef,
    policy: &ProductReleasePolicy,
    target: &ReleaseTargetPolicy,
) -> Result<(ReleaseManifest, Vec<u8>, PathBuf), String> {
    let manifest_bytes = fetch_release_bytes(&artifact.manifest_uri).await?;
    if release_control::sha256_bytes(&manifest_bytes) != artifact.manifest_sha256 {
        return Err("release manifest digest does not match desired state".to_string());
    }
    let manifest: ReleaseManifest = serde_json::from_slice(&manifest_bytes)
        .map_err(|error| format!("invalid release manifest: {error}"))?;
    release_control::validate_manifest(&manifest)?;
    if manifest.product != product
        || manifest.version != desired.version
        || manifest.platform != target.platform
        || manifest.artifact_sha256 != artifact.artifact_sha256
        || manifest.source_revision != artifact.source_revision
        || manifest.key_id != artifact.key_id
        || manifest.binary != policy.binary
        || manifest.launcher != policy.launcher
        || manifest.config_schema != policy.config_schema
        || manifest.state_schema != policy.state_schema
    {
        return Err("release manifest does not match registry desired state".to_string());
    }
    if manifest.qualification.status != QualificationStatus::Passed {
        return Err("release candidate has not passed qualification".to_string());
    }
    if crate::release::version_newer(env!("CARGO_PKG_VERSION"), &manifest.minimum_stado_version) {
        return Err(format!(
            "release requires Stado {}, host runs {}",
            manifest.minimum_stado_version,
            env!("CARGO_PKG_VERSION")
        ));
    }
    let signature = fetch_release_bytes(&artifact.signature_uri).await?;
    let signature = std::str::from_utf8(&signature)
        .map_err(|_| "release signature is not UTF-8".to_string())?;
    let public_key = control
        .trusted_keys
        .get(&artifact.key_id)
        .ok_or_else(|| "release signing key is not trusted by registry".to_string())?;
    release_control::verify_manifest(public_key, &manifest, signature)?;
    let archive = fetch_release_bytes(&artifact.archive_uri).await?;
    if archive.len() as u64 != manifest.artifact_bytes
        || release_control::sha256_bytes(&archive) != manifest.artifact_sha256
    {
        return Err("release archive does not match its signed manifest".to_string());
    }
    let directory = release_control::install_directory(policy, target, &manifest);
    Ok((manifest, archive, directory))
}

fn marker_path(directory: &Path) -> PathBuf {
    directory.join(".stado-release.json")
}

fn stage_release(
    manifest: &ReleaseManifest,
    archive: &[u8],
    directory: &Path,
) -> Result<(), String> {
    let manifest_sha =
        release_control::sha256_bytes(&release_control::canonical_manifest(manifest)?);
    if directory.exists() {
        let marker = std::fs::read(marker_path(directory)).map_err(|_| {
            format!(
                "immutable release directory has no marker: {}",
                directory.display()
            )
        })?;
        let installed: ReleaseManifest = serde_json::from_slice(&marker).map_err(|_| {
            format!(
                "immutable release marker is invalid: {}",
                directory.display()
            )
        })?;
        if release_control::sha256_bytes(&release_control::canonical_manifest(&installed)?)
            != manifest_sha
        {
            return Err(format!(
                "immutable release directory contains a different manifest: {}",
                directory.display()
            ));
        }
        return Ok(());
    }
    release_control::safe_extract_archive(archive, directory)?;
    atomic_json(&marker_path(directory), manifest)
}

/// Resolve the executable from the exact release the local agent currently
/// records and routes as active. Desired state is deliberately irrelevant:
/// a rejected newer candidate may be quarantined while its healthy predecessor
/// remains the release actually serving the stable bind.
pub(crate) fn active_binary(
    product: &str,
    target_name: &str,
    policy: &ProductReleasePolicy,
    target: &ReleaseTargetPolicy,
) -> Result<ActiveBinary, String> {
    let state = load_state(target, product, target_name)?;
    let active = state.active.as_ref().ok_or_else(|| {
        format!(
            "{product} is release-controlled on {target_name} but has no observed active release (phase {:?})",
            state.phase
        )
    })?;
    let directory = PathBuf::from(&active.release_dir);
    let install_root = release_control::install_root_path(policy, target);
    let install_root = install_root
        .to_str()
        .ok_or_else(|| format!("{product} install root is not valid UTF-8"))?;
    let active_process_matches = release_processes(install_root).into_iter().any(|process| {
        process.pid == active.pid
            && process.version == active.version
            && process.port == Some(active.port)
            && process.release_dir == directory
    });
    if !active_process_matches {
        return Err(format!(
            "{product} observed active process tuple pid={} version={} port={} release_dir={} does not match a live release process",
            active.pid,
            active.version,
            active.port,
            directory.display()
        ));
    }
    if state.quarantined.contains_key(&active.artifact_sha256) {
        return Err(format!(
            "{product} observed active digest {} is quarantined",
            active.artifact_sha256
        ));
    }

    let serving = target.blue_green_serving()?;
    let proxy_pid = state
        .proxy_pid
        .ok_or_else(|| format!("{product} observed active release has no recorded stable proxy"))?;
    if !proxy_process_matches(proxy_pid, target, &serving, product)? {
        return Err(format!(
            "{product} recorded stable proxy pid {proxy_pid} does not match the exact executable and arguments"
        ));
    }
    let proxy_path = proxy_state_path(target, product);
    let proxy: ProxyState =
        serde_json::from_slice(&std::fs::read(&proxy_path).map_err(|error| {
            format!("cannot read proxy target {}: {error}", proxy_path.display())
        })?)
        .map_err(|error| format!("invalid proxy target {}: {error}", proxy_path.display()))?;
    let expected_upstream = format!("127.0.0.1:{}", active.port);
    if proxy.generation != state.rollout_generation || proxy.upstream != expected_upstream {
        return Err(format!(
            "{product} stable proxy targets generation {} upstream {}, not observed active generation {} upstream {expected_upstream}",
            proxy.generation, proxy.upstream, state.rollout_generation
        ));
    }

    let marker_path = marker_path(&directory);
    let marker = std::fs::read(&marker_path).map_err(|error| {
        format!(
            "cannot read active release marker {}: {error}",
            marker_path.display()
        )
    })?;
    let manifest: ReleaseManifest = serde_json::from_slice(&marker)
        .map_err(|error| format!("active release marker is invalid: {error}"))?;
    release_control::validate_manifest(&manifest)?;
    let manifest_sha =
        release_control::sha256_bytes(&release_control::canonical_manifest(&manifest)?);
    if manifest_sha != active.manifest_sha256
        || manifest.product != product
        || manifest.version != active.version
        || manifest.platform != target.platform
        || manifest.artifact_sha256 != active.artifact_sha256
        || manifest.binary != policy.binary
        || manifest.qualification.status != QualificationStatus::Passed
    {
        return Err(format!(
            "{product} observed active release marker does not match its process identity"
        ));
    }
    let expected_directory = release_control::install_directory(policy, target, &manifest);
    if directory != expected_directory {
        return Err(format!(
            "{product} active release directory {} is not the policy-derived directory {}",
            directory.display(),
            expected_directory.display()
        ));
    }
    let path = directory.join(&policy.binary);
    let metadata = std::fs::symlink_metadata(&path)
        .map_err(|error| format!("cannot inspect active binary {}: {error}", path.display()))?;
    if !metadata.is_file()
        || metadata.file_type().is_symlink()
        || metadata.permissions().mode() & 0o111 == 0
    {
        return Err(format!(
            "active binary is not an executable regular file: {}",
            path.display()
        ));
    }
    Ok(ActiveBinary {
        path,
        version: manifest.version,
        platform: manifest.platform,
        artifact_sha256: manifest.artifact_sha256,
        manifest_sha256: manifest_sha,
    })
}

/// The port the proxy currently forwards to, read from its own target file.
///
/// When the state file has lost its `active` record -- interrupted rollouts and an
/// orphaned reconciler both did that this week -- the proxy's target is the only
/// truthful statement of which port carries traffic.
fn proxy_upstream_port(target: &ReleaseTargetPolicy, product: &str) -> Option<u16> {
    let raw = std::fs::read(proxy_state_path(target, product)).ok()?;
    let value: serde_json::Value = serde_json::from_slice(&raw).ok()?;
    value
        .get("upstream")?
        .as_str()?
        .rsplit(':')
        .next()?
        .parse()
        .ok()
}

#[derive(Debug)]
struct ReleaseProcess {
    pid: i32,
    version: String,
    port: Option<u16>,
    release_dir: PathBuf,
}

/// Every process running out of this product's `releases/` directory, including
/// the immutable release directory named by its exact argument vector.
///
/// These processes are all children of some run of this agent -- nothing else
/// executes from that directory -- so any of them the state file does not name is
/// a leak from a run that died between spawning and recording.
fn release_processes(install_root: &str) -> Vec<ReleaseProcess> {
    let output = match std::process::Command::new("/bin/ps")
        .args(["-eo", "pid=,command="])
        .output()
    {
        Ok(output) => output,
        Err(_) => return Vec::new(),
    };
    let marker = format!("{}/releases/", install_root.trim_end_matches('/'));
    let mut found = Vec::new();
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        if !line.contains(&marker) {
            continue;
        }
        let mut fields = line.split_whitespace();
        let Some(pid) = fields.next().and_then(|first| first.parse::<i32>().ok()) else {
            continue;
        };
        let arguments: Vec<&str> = fields.collect();
        let Some((version, release_dir)) = arguments.iter().find_map(|argument| {
            let path = argument
                .split_once('=')
                .map_or(*argument, |(_, value)| value);
            let (prefix, tail) = path.split_once(&marker)?;
            let mut components = tail.split('/');
            let version = components.next()?;
            let platform = components.next()?;
            if version.is_empty() || platform.is_empty() {
                return None;
            }
            Some((
                version.to_string(),
                PathBuf::from(format!("{prefix}{marker}{version}/{platform}")),
            ))
        }) else {
            continue;
        };
        let mut port = None;
        for pair in arguments.windows(2) {
            if pair[0] == "--port" {
                port = pair[1].parse().ok();
            }
        }
        found.push(ReleaseProcess {
            pid,
            version,
            port,
            release_dir,
        });
    }
    found
}
async fn stable_bind_ready(serving: &BlueGreenServing) -> bool {
    let url = format!("http://{}{}", serving.stable_bind, serving.readiness_path);
    reqwest::Client::new()
        .get(url)
        .timeout(Duration::from_secs(3))
        .send()
        .await
        .is_ok_and(|response| response.status().is_success())
}

/// Reconcile a proxy that survived an agent handoff before its pid reached the
/// host state document.
///
/// The exact proxy argv belongs to this product and runs under the same
/// per-product reconcile lock as the state update. A live upstream is the crash
/// window: adopt the proxy rather than interrupting traffic. With no owned
/// release process and a dead upstream, the proxy can only pin the stable bind
/// to nowhere; retire it and let the declared legacy service reclaim the bind.
async fn reconcile_stable_proxy(
    target: &ReleaseTargetPolicy,
    product: &str,
    install_root: &str,
    readiness_timeout_seconds: u64,
    state: &mut HostReleaseState,
) -> Result<(), String> {
    let serving = target.blue_green_serving()?;
    let Some(proxy_pid) = exact_proxy_pid(target, &serving, product)? else {
        return Ok(());
    };
    let upstream = proxy_upstream_port(target, product);
    let upstream_is_owned = upstream.is_some_and(|upstream_port| {
        release_processes(install_root)
            .iter()
            .any(|process| process.port == Some(upstream_port))
    });
    if upstream_is_owned && stable_bind_ready(&serving).await {
        state.proxy_pid = Some(proxy_pid);
        save_state(target, state)?;
        eprintln!(
            "adopted {product} release proxy pid={proxy_pid} after interrupted handoff; \
             upstream {upstream:?} is ready"
        );
        return Ok(());
    }

    let ownership_empty =
        state.active.is_none() && state.candidate.is_none() && state.previous.is_none();
    if !ownership_empty {
        return Ok(());
    }
    let _ = kill(Pid::from_raw(proxy_pid), Signal::SIGTERM);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while pid_alive(proxy_pid) {
        if tokio::time::Instant::now() >= deadline {
            return Err(format!(
                "orphaned {product} release proxy pid={proxy_pid} did not exit"
            ));
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    restore_legacy(target)?;
    if target.legacy_launchd_plist.is_some() {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(readiness_timeout_seconds);
        while !stable_bind_ready(&serving).await {
            if tokio::time::Instant::now() >= deadline {
                return Err(format!(
                    "legacy {product} did not reclaim {} after orphaned proxy retirement",
                    serving.stable_bind
                ));
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    }
    state.proxy_pid = None;
    save_state(target, state)?;
    eprintln!(
        "retired orphaned {product} release proxy pid={proxy_pid}; upstream \
         {upstream:?} had no ready owned release"
    );
    Ok(())
}

/// Terminate release processes the state file does not know about.
///
/// The agent owned every one of them once; a rollout that died between spawn and
/// save leaves the process running and the record absent, and the agent trusted
/// only the record. On the always-on Mac that produced a candidate from three
/// releases ago holding a candidate port for hours, a rollout that could never
/// bind past it, and an operator asked to stop processes by hand -- which is not a
/// release system. The one process spared is whichever serves the proxy's current
/// upstream: it carries traffic, and the normal cutover retires it by routing away
/// first, after which the next pass sweeps it here.
fn sweep_leaked_processes(
    target: &ReleaseTargetPolicy,
    product: &str,
    install_root: &str,
    state: &HostReleaseState,
) {
    let mut known = Vec::new();
    for record in [&state.active, &state.candidate, &state.previous]
        .into_iter()
        .flatten()
    {
        known.push(record.pid);
    }
    if let Some(pid) = state.proxy_pid {
        known.push(pid);
    }
    let upstream = proxy_upstream_port(target, product);
    for process in release_processes(install_root) {
        if known.contains(&process.pid) {
            continue;
        }
        if upstream.is_some() && process.port == upstream {
            eprintln!(
                "leaked {product} {} pid={} still carries traffic on \
                 {:?}; retiring by cutover, not by kill",
                process.version, process.pid, process.port
            );
            continue;
        }
        let _ = kill(Pid::from_raw(process.pid), Signal::SIGTERM);
        eprintln!(
            "swept leaked {product} {} pid={} port={:?}",
            process.version, process.pid, process.port
        );
    }
}

fn next_port(candidate_ports: [u16; 2], state: &HostReleaseState, occupied: Option<u16>) -> u16 {
    // With no active record the previous rule always chose the first candidate
    // port -- exactly where a de-facto active from a lost rollout is still
    // serving, so the new candidate died on the bind and the rollout could never
    // proceed. The proxy's upstream is authoritative when the record is silent.
    match state.active.as_ref().map(|active| active.port).or(occupied) {
        Some(port) if port == candidate_ports[0] => candidate_ports[1],
        _ => candidate_ports[0],
    }
}

/// Canonical object URI of one host's rollout status, inside this
/// deployment's own namespace and its declared `system/` prefix.
///
/// The literal `stado://system/...` this replaced named a namespace no grant
/// declares, so every publish answered 401. Resolving through `ObjectRef`
/// keeps writer and reader on one path whether they reach the store through
/// the object API or read the co-located disk directly.
pub fn release_status_uri(product: &str, target: &str) -> String {
    let namespace = crate::config::wc_stado_storage_namespace();
    format!("stado://{namespace}/system/release-status/{product}/{target}.json")
}

async fn publish_status(state: &HostReleaseState) -> Result<(), String> {
    let status = PublishedStatus {
        schema_version: STATUS_SCHEMA,
        product: &state.product,
        target: &state.target,
        rollout_generation: state.rollout_generation,
        phase: state.phase,
        active_version: state.active.as_ref().map(|record| record.version.as_str()),
        active_sha256: state
            .active
            .as_ref()
            .map(|record| record.artifact_sha256.as_str()),
        previous_version: state
            .previous
            .as_ref()
            .map(|record| record.version.as_str()),
        detail: &state.detail,
        updated_at: state.updated_at,
    };
    let temporary = tempfile::NamedTempFile::new()
        .map_err(|error| format!("cannot create release status staging: {error}"))?;
    std::fs::write(
        temporary.path(),
        serde_json::to_vec(&status).map_err(|error| error.to_string())?,
    )
    .map_err(|error| format!("cannot write release status staging: {error}"))?;
    crate::cli::storage::store_object(
        &release_status_uri(&state.product, &state.target),
        &temporary.path().display().to_string(),
        "application/json",
        false,
    )
    .await
    .map(|_| ())
    .map_err(|error| error.to_string())
}

/// Publish the committed outcome of a generic managed-service activation into
/// the same status document `stado release status` reads.
// The status document is a flat wire contract; keeping its fields explicit
// makes accidental schema changes visible at every publication call.
#[allow(clippy::too_many_arguments)]
pub async fn publish_service_release_status(
    product: &str,
    target: &str,
    rollout_generation: u64,
    phase: RolloutPhase,
    active_version: Option<&str>,
    active_sha256: Option<&str>,
    previous_version: Option<&str>,
    detail: &str,
) -> Result<(), String> {
    let status = PublishedStatus {
        schema_version: STATUS_SCHEMA,
        product,
        target,
        rollout_generation,
        phase,
        active_version,
        active_sha256,
        previous_version,
        detail,
        updated_at: Utc::now(),
    };
    let temporary = tempfile::NamedTempFile::new()
        .map_err(|error| format!("cannot create service release status staging: {error}"))?;
    std::fs::write(
        temporary.path(),
        serde_json::to_vec(&status).map_err(|error| error.to_string())?,
    )
    .map_err(|error| format!("cannot write service release status staging: {error}"))?;
    crate::cli::storage::store_object(
        &release_status_uri(product, target),
        &temporary.path().display().to_string(),
        "application/json",
        false,
    )
    .await
    .map(|_| ())
    .map_err(|error| error.to_string())
}

async fn rollback(
    target: &ReleaseTargetPolicy,
    state: &mut HostReleaseState,
    reason: String,
) -> Result<(), String> {
    let failed = state.active.take().or_else(|| state.candidate.take());
    if let Some(record) = &failed {
        state.quarantined.insert(
            record.artifact_sha256.clone(),
            QuarantineRecord::new(reason.clone()),
        );
    }
    if let Some(previous) = state.previous.take() {
        let serving = target.blue_green_serving()?;
        let product = state.product.clone();
        let generation = state.rollout_generation;
        ensure_active_proxy(target, &serving, &product, generation, &previous, state).await?;
        if let Some(record) = &failed {
            terminate(record);
        }
        state.active = Some(previous);
    } else {
        if let Some(proxy_pid) = state.proxy_pid.take() {
            let _ = kill(Pid::from_raw(proxy_pid), Signal::SIGTERM);
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
        restore_legacy(target)?;
        if let Some(record) = &failed {
            terminate(record);
        }
    }
    state.candidate = None;
    state.phase = RolloutPhase::RolledBack;
    state.detail = reason;
    state.cutover_at = None;
    save_state(target, state)
}

/// How many consecutive quarantines sharing one named cause count as a wall
/// rather than a bad attempt.
///
/// Chosen by measuring the live data, not by taste. Classifying all twenty
/// `brama` records on `charless-mac-mini` gives a longest run of one classified
/// cause of **two** -- `brama-0.2.42` and `brama-0.2.43`, four days apart, both
/// refused at capability redemption. Two is ordinary: a candidate fails,
/// someone changes something, the next candidate fails the same way because the
/// change was wrong. Refusing at two would block that loop on its first honest
/// iteration.
///
/// Three is therefore the smallest threshold that fires on nothing in a month
/// of real history -- it raises no refusal anywhere in those twenty records --
/// while catching the first step past the worst run the fleet has actually
/// produced. Calibrating it against the data rather than the anecdote matters:
/// the 2026-09-01 sequence looks like a run of three and is not one, because
/// 0.2.49, 0.2.50 and 0.2.51 wrote no failure line and cannot be named.
///
/// The reason no historical window trips this is that twelve of the twenty rows
/// are unclassified. The threshold is worth having anyway, because from here on
/// a cause is recorded at the moment of quarantine from the whole log, so runs
/// become visible instead of being invisible in a column that did not exist.
const REPEAT_CAUSE_LIMIT: usize = 3;

/// Is this product about to walk into a wall it has already walked into?
///
/// `None` when there is no such wall.
///
/// **The "has anything changed" test.** The most recent [`REPEAT_CAUSE_LIMIT`]
/// quarantines must all name the same classified cause, and that is the whole
/// test, because of what it takes for a record to leave this map. The agent
/// only ever adds; the one thing that removes an entry is
/// `stado release quarantine clear --digest ... --reason ...`, which is an
/// operator stating, on the audit trail beside this file, that something
/// changed. So a run that is still intact *is* the assertion that nothing has
/// changed, and it needs no extra state to record.
///
/// A new digest is deliberately not a change. That is the exact mistake the
/// incident made: new digest, new version number, same unserved credential, and
/// every rollout treated the new digest as a new situation.
///
/// Consecutive, not "the last three classified": an unclassified quarantine
/// between two members is a candidate that failed in a way this agent could not
/// match to the others, and claiming it as more of the same is precisely the
/// overreach the cause vocabulary exists to avoid. It breaks the run.
///
/// Three ways out, none of them new and none of them a bypass flag:
///
/// - clear any one of the run's digests, which is the audited override and
///   immediately shortens the run below the limit;
/// - promote a candidate that fails for a *different* cause, which breaks the
///   run on its own;
/// - fix the cause, after which nothing quarantines and the run stops growing.
///
/// [`QuarantineCause::Unclassified`] never triggers this. Twelve of the twenty
/// live records are unclassified, seven of them consecutively, and refusing on a
/// cause the agent could not name would have frozen this product for a month on
/// no evidence at all.
pub fn cause_run(state: &HostReleaseState) -> Option<CauseRun> {
    let mut recent: Vec<&QuarantineRecord> = state.quarantined.values().collect();
    // The map is keyed by digest, so its own order is the digest's. Recency is
    // the question being asked.
    recent.sort_by_key(|record| std::cmp::Reverse(record.quarantined_at));
    let cause = recent.first()?.cause;
    if !cause.is_classified() {
        return None;
    }
    let run: Vec<&QuarantineRecord> = recent
        .into_iter()
        .take_while(|record| record.cause == cause)
        .collect();
    Some(CauseRun {
        cause,
        evidence: run[0].evidence.clone(),
        since: run[run.len() - 1].quarantined_at,
        digests: state
            .quarantined
            .iter()
            .filter(|(_, record)| run.iter().any(|member| std::ptr::eq(*member, *record)))
            .map(|(digest, _)| digest.clone())
            .collect(),
    })
}

/// The most recent quarantines that all failed one named way.
///
/// The run is however long it actually is — one row is a run of one — because
/// the length is no longer the whole decision. It is the input to two different
/// questions: what does the cause's own condition say right now, and failing
/// that, is this repetitive enough to stop on.
#[derive(Debug, Clone)]
pub struct CauseRun {
    pub cause: QuarantineCause,
    /// The decisive line from the most recent member of the run.
    pub evidence: String,
    /// When the oldest member of the run was quarantined.
    pub since: DateTime<Utc>,
    /// Every digest in the run, so the override names a real digest.
    pub digests: Vec<String>,
}

impl CauseRun {
    pub fn len(&self) -> usize {
        self.digests.len()
    }

    pub fn is_empty(&self) -> bool {
        self.digests.is_empty()
    }

    /// Would counting alone stop the next candidate?
    ///
    /// The unchanged fallback: [`REPEAT_CAUSE_LIMIT`] consecutive quarantines
    /// of one classified cause. This is what governs every cause with no
    /// checkable condition, and what governs a cause whose condition could not
    /// be reached.
    pub fn repeats(&self) -> bool {
        self.len() >= REPEAT_CAUSE_LIMIT
    }
}

/// Why the agent is holding, in the words of whatever it actually established.
///
/// A refusal that says only that it fired leaves the operator to guess whether
/// the wall was seen or merely inferred, and those call for different next
/// moves: one is repaired, the other is investigated.
#[derive(Debug, Clone)]
pub enum HoldGround {
    /// The cause's own condition was asked and still reports the wall. The
    /// strongest ground there is, and it holds at the FIRST quarantine.
    Observed {
        /// The check that was run, as an operator would run it.
        check: String,
        /// The vault's own sentence for what still refuses.
        detail: String,
        /// When the check was run — now, not when the candidate failed.
        at: DateTime<Utc>,
    },
    /// No condition to ask, or it could not answer, and the run is long enough
    /// to stop on by itself.
    Repeated {
        count: usize,
        since: DateTime<Utc>,
        /// Present when a condition exists but could not be reached, so the
        /// refusal does not imply the count was the only available evidence.
        unreachable: Option<String>,
    },
}

/// A hold on the next candidate, with the ground it rests on.
#[derive(Debug, Clone)]
pub struct CauseHold {
    pub cause: QuarantineCause,
    pub evidence: String,
    pub digests: Vec<String>,
    pub ground: HoldGround,
}

impl CauseHold {
    /// The sentence recorded on the host and printed to the operator.
    ///
    /// Ends with the override, always. A refusal that does not say how to
    /// overrule it is a refusal an operator works around by editing the state
    /// file, which is the unaudited write this whole area exists to remove.
    pub fn sentence(&self) -> String {
        let mut sentence = match &self.ground {
            HoldGround::Observed { check, detail, at } => format!(
                "refusing to promote another candidate: {} still refuses. Checked with `{check}` \
                 at {}, which reported: {detail}.",
                self.cause.as_str(),
                at.to_rfc3339(),
            ),
            HoldGround::Repeated {
                count,
                since,
                unreachable,
            } => {
                let mut text = format!(
                    "refusing to promote another candidate: the last {count} quarantines on this \
                     host all failed for {} since {}, and nothing about it has changed. {}",
                    self.cause.as_str(),
                    since.to_rfc3339(),
                    self.evidence
                );
                if let Some(why) = unreachable {
                    text.push_str(&format!(
                        " The condition behind this cause could not be checked ({why}), so this \
                         rests on the repetition rather than on an observation."
                    ));
                }
                text
            }
        };
        if let Some(remedy) = self.cause.remedy() {
            sentence.push_str(&format!(" Remedy: {remedy}."));
        }
        sentence.push_str(&format!(
            " Override by retiring one of these digests with: stado release quarantine clear \
             --digest {} --reason <text>.",
            self.digests.first().map_or("<digest>", String::as_str)
        ));
        sentence
    }
}

/// How long the condition check gets before it counts as no answer.
///
/// It opens vault items, which is one `gpg` per distinct item, so it is not
/// instant — but it is scoped to one resource, and a check that outlives this
/// is a check that is not going to answer. A hung predicate must degrade to
/// [`WallVerdict::Unknown`] rather than stall a reconcile tick.
const PREDICATE_TIMEOUT_SECONDS: u64 = 20;

/// Ask a cause's own condition whether its wall still stands.
///
/// Runs as the release user with that user's `HOME`, mirroring
/// [`spawn_release`], because the vault the check reads belongs to that account
/// and a check run as the wrong user reads the wrong store. `PATH` carries the
/// Homebrew prefix for the same reason [`crate::cli::service`]'s owner read
/// does: the decrypt helper lives there, and without it every answer would be
/// an unreachable one.
///
/// Strictly read-only. `routes verify` resolves and reports; it starts nothing,
/// writes nothing, and is safe against a live broker — which is why it is a
/// predicate at all.
async fn ask_wall(
    target: &ReleaseTargetPolicy,
    predicate: &release_cause::CausePredicate,
) -> (release_cause::WallVerdict, Option<String>) {
    let unreachable = |why: String| (release_cause::WallVerdict::Unknown, Some(why));
    let skarbiec = Path::new(&target.home).join(".stado/bin/skarbiec");
    if !skarbiec.is_file() {
        return unreachable(format!("no skarbiec binary at {}", skarbiec.display()));
    }
    let mut command = tokio::process::Command::new("/usr/bin/sudo");
    command
        .args(["-n", "-u", &target.run_as_user, "-H", "/usr/bin/env"])
        .arg(format!("HOME={}", target.home))
        .arg("PATH=/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin")
        .arg(&skarbiec)
        .args(&predicate.args)
        .stdin(Stdio::null());
    let run = tokio::time::timeout(
        Duration::from_secs(PREDICATE_TIMEOUT_SECONDS),
        command.output(),
    )
    .await;
    let output = match run {
        Err(_) => {
            return unreachable(format!(
                "`skarbiec {}` did not answer within {PREDICATE_TIMEOUT_SECONDS}s",
                predicate.args.join(" ")
            ))
        }
        Ok(Err(error)) => {
            return unreachable(format!("cannot run {}: {error}", skarbiec.display()))
        }
        Ok(Ok(output)) => output,
    };
    let stdout = String::from_utf8_lossy(&output.stdout);
    let verdict = release_cause::read_routes_verify(output.status.success(), &stdout);
    let note = match verdict {
        release_cause::WallVerdict::Unknown => Some(
            String::from_utf8_lossy(&output.stderr)
                .trim()
                .lines()
                .next_back()
                .unwrap_or("the check reported nothing")
                .chars()
                .take(200)
                .collect(),
        ),
        _ => release_cause::routes_verify_detail(&stdout),
    };
    (verdict, note)
}

/// Should the agent spend another candidate, and if not, on what ground?
///
/// The order is the whole change. Ask the condition first; fall back to
/// counting only when there is no condition to ask or it could not answer:
///
/// - [`WallVerdict::Present`] holds at the FIRST quarantine of that cause. The
///   wall was observed, so a second and third candidate would establish
///   nothing that is not already known.
/// - [`WallVerdict::Gone`] releases, including a run past
///   [`REPEAT_CAUSE_LIMIT`]. This is the property counting cannot have: an
///   operator who refills the credential gets promotion back because the check
///   stops failing, with no override and nothing to remember.
/// - [`WallVerdict::Unknown`] decides nothing by itself and never releases a
///   hold. It falls through to the count, which is exactly the behaviour before
///   any of this existed, and the refusal says the check was unreachable so the
///   ground is not mistaken for an observation.
async fn cause_hold(target: &ReleaseTargetPolicy, state: &HostReleaseState) -> Option<CauseHold> {
    let run = cause_run(state)?;
    let repeated = |unreachable: Option<String>| {
        run.repeats().then(|| CauseHold {
            cause: run.cause,
            evidence: run.evidence.clone(),
            digests: run.digests.clone(),
            ground: HoldGround::Repeated {
                count: run.len(),
                since: run.since,
                unreachable,
            },
        })
    };
    let Some(predicate) = run.cause.predicate(&run.evidence) else {
        return repeated(None);
    };
    match ask_wall(target, &predicate).await {
        (release_cause::WallVerdict::Present, detail) => Some(CauseHold {
            cause: run.cause,
            evidence: run.evidence.clone(),
            digests: run.digests.clone(),
            ground: HoldGround::Observed {
                check: format!("skarbiec {}", predicate.args.join(" ")),
                detail: detail.unwrap_or_else(|| run.evidence.clone()),
                at: Utc::now(),
            },
        }),
        (release_cause::WallVerdict::Gone, _) => None,
        (release_cause::WallVerdict::Unknown, why) => repeated(why),
    }
}

async fn reconcile_product(
    control: &ReleaseControl,
    product: &str,
    policy: &ProductReleasePolicy,
    target_name: &str,
    target: &ReleaseTargetPolicy,
) -> Result<HostReleaseState, String> {
    let mut state = load_state(target, product, target_name)?;
    let install_root = release_control::install_root_path(policy, target);
    let install_root = install_root
        .to_str()
        .ok_or_else(|| format!("{product} install root is not valid UTF-8"))?;
    // Repair the stable bind before any desired/quarantine branch can return.
    // `reconcile_once` holds the per-product lock across this state load,
    // declaration/world reconciliation, and every persisted repair below.
    reconcile_stable_proxy(
        target,
        product,
        install_root,
        policy.strategy.readiness_timeout_seconds,
        &mut state,
    )
    .await?;
    // `reconcile_once` hands only blue-green policies to this function; ask
    // for the serving coordinates by name rather than re-checking the
    // validator's invariant, so a replace policy reaching here fails loudly
    // instead of halfway through a rollout.
    let serving = target.blue_green_serving()?;
    // Reconcile the process world before reasoning from the record: anything
    // running out of this product's releases directory that the record does not
    // name is a leak from a run that died between spawning and saving.
    sweep_leaked_processes(target, product, install_root, &state);
    let Some(desired) = policy.desired.as_ref() else {
        state.phase = RolloutPhase::Idle;
        state.detail = "no desired release".to_string();
        save_state(target, &mut state)?;
        return Ok(state);
    };
    let artifact = desired
        .artifacts
        .get(&target.platform)
        .ok_or_else(|| format!("desired release has no {} artifact", target.platform))?;
    let repeats_failed_rollout = state.phase == RolloutPhase::RolledBack
        && state.rollout_generation == desired.rollout_generation
        && state.active.is_none()
        && state.previous.is_none();
    state.rollout_generation = desired.rollout_generation;
    if repeats_failed_rollout {
        state.quarantined.insert(
            artifact.artifact_sha256.clone(),
            QuarantineRecord::new(state.detail.clone()),
        );
        state.phase = RolloutPhase::Quarantined;
        state.detail =
            "desired release digest is quarantined after its previous rollback".to_string();
        save_state(target, &mut state)?;
        return Ok(state);
    }

    if state.quarantined.contains_key(&artifact.artifact_sha256) {
        if let Some(active) = state.active.clone() {
            if let Err(reason) = ensure_active_proxy(
                target,
                &serving,
                product,
                desired.rollout_generation,
                &active,
                &mut state,
            )
            .await
            {
                if policy.strategy.automatic_rollback {
                    rollback(target, &mut state, reason).await?;
                } else {
                    state.phase = RolloutPhase::Failed;
                    state.detail = reason;
                    save_state(target, &mut state)?;
                }
                return Ok(state);
            }
        }
        state.phase = RolloutPhase::Quarantined;
        state.detail = "desired release digest is quarantined on this host".to_string();
        save_state(target, &mut state)?;
        return Ok(state);
    }

    if state
        .active
        .as_ref()
        .is_some_and(|active| active.artifact_sha256 == artifact.artifact_sha256)
    {
        let active = state.active.clone().expect("checked above");
        let proxy_result = ensure_active_proxy(
            target,
            &serving,
            product,
            desired.rollout_generation,
            &active,
            &mut state,
        )
        .await;
        if let Err(reason) = proxy_result {
            if policy.strategy.automatic_rollback {
                rollback(target, &mut state, reason).await?;
            } else {
                state.phase = RolloutPhase::Failed;
                state.detail = reason;
                save_state(target, &mut state)?;
            }
            return Ok(state);
        }
        if !matches!(
            state.phase,
            RolloutPhase::Monitoring | RolloutPhase::Committed
        ) {
            state.phase = RolloutPhase::Routed;
            state.detail = format!("stable proxy routed to candidate port {}", active.port);
            state.cutover_at.get_or_insert_with(Utc::now);
            save_state(target, &mut state)?;
            tokio::time::sleep(Duration::from_secs(policy.strategy.drain_timeout_seconds)).await;
            if !ready(&active, &serving.readiness_path).await {
                if policy.strategy.automatic_rollback {
                    rollback(
                        target,
                        &mut state,
                        "candidate failed during drain".to_string(),
                    )
                    .await?;
                } else {
                    state.phase = RolloutPhase::Failed;
                    state.detail =
                        "candidate failed during drain; automatic rollback is disabled".to_string();
                    save_state(target, &mut state)?;
                }
                return Ok(state);
            }
            state.phase = RolloutPhase::Monitoring;
            state.detail = "previous release drained and retained for rollback window".to_string();
            save_state(target, &mut state)?;
        }
        if state.phase == RolloutPhase::Monitoring {
            let elapsed = state
                .cutover_at
                .map(|cutover| {
                    Utc::now()
                        .signed_duration_since(cutover)
                        .num_seconds()
                        .max(0) as u64
                })
                .unwrap_or_default();
            if elapsed >= policy.strategy.rollback_window_seconds {
                if let Some(previous) = state.previous.take() {
                    terminate(&previous);
                }
                state.phase = RolloutPhase::Committed;
                state.detail = "release committed after rollback window".to_string();
                save_state(target, &mut state)?;
            }
        }
        return Ok(state);
    }

    // Everything below stages and burns a new candidate. Before spending one,
    // ask the cause's own condition whether the wall is still there, and fall
    // back to counting only when there is nothing to ask or it cannot answer.
    // This sits AFTER the desired-digest quarantine guard above, which returns
    // first and is untouched.
    if let Some(hold) = cause_hold(target, &state).await {
        state.phase = RolloutPhase::Quarantined;
        state.detail = hold.sentence();
        save_state(target, &mut state)?;
        return Ok(state);
    }

    if let Some(incomplete) = state.candidate.take() {
        terminate(&incomplete);
        state.detail = "discarded incomplete candidate from an interrupted rollout".to_string();
        save_state(target, &mut state)?;
    }

    state.phase = RolloutPhase::Downloaded;
    state.detail = format!("fetching {} {}", product, desired.version);
    save_state(target, &mut state)?;
    let (manifest, archive, directory) =
        match fetch_candidate(control, product, desired, artifact, policy, target).await {
            Ok(candidate) => candidate,
            Err(reason) => {
                state.quarantined.insert(
                    artifact.artifact_sha256.clone(),
                    QuarantineRecord::new(reason.clone()),
                );
                state.phase = RolloutPhase::Quarantined;
                state.detail = reason;
                save_state(target, &mut state)?;
                return Ok(state);
            }
        };
    state.phase = RolloutPhase::Verified;
    state.detail = "signature, provenance, qualification, schema, and digest verified".to_string();
    save_state(target, &mut state)?;

    if let Some(active) = &state.active {
        if !manifest
            .rollback_compatible_with
            .iter()
            .any(|version| version == &active.version)
        {
            let reason = format!(
                "release {} does not declare rollback compatibility with {}",
                manifest.version, active.version
            );
            state.quarantined.insert(
                artifact.artifact_sha256.clone(),
                QuarantineRecord::new(reason.clone()),
            );
            state.phase = RolloutPhase::Quarantined;
            state.detail = reason;
            save_state(target, &mut state)?;
            return Ok(state);
        }
    }

    stage_release(&manifest, &archive, &directory)?;
    state.phase = RolloutPhase::Staged;
    state.detail = format!("staged immutable release at {}", directory.display());
    save_state(target, &mut state)?;

    let port = next_port(
        serving.candidate_ports,
        &state,
        proxy_upstream_port(target, product),
    );
    let process = spawn_release(product, policy, target, &manifest, &directory, port)?;
    state.candidate = Some(process.clone());
    state.phase = RolloutPhase::CandidateRunning;
    state.detail = format!("candidate pid={} port={port}", process.pid);
    save_state(target, &mut state)?;
    if let Some(why) = await_ready_because(
        &process,
        &serving.readiness_path,
        policy.strategy.readiness_timeout_seconds,
    )
    .await
    {
        terminate(&process);
        let stderr = log_evidence(
            &release_log_path(target, product, &manifest.version, "err"),
            20,
            1200,
        );
        let stdout = log_evidence(
            &release_log_path(target, product, &manifest.version, "out"),
            5,
            400,
        );
        let reason = format!(
            "candidate did not become ready within {}s: {why}; stderr {}; stdout {}",
            policy.strategy.readiness_timeout_seconds, stderr.rendered, stdout.rendered
        );
        // Classified from every byte the candidate wrote, not from the bounded
        // tail that went into the reason, so a name is never withheld because
        // the quote was trimmed. On the live records the two happen to agree;
        // that is luck about where those products put their decisive line, not
        // a property worth depending on, and the whole log costs one read that
        // has already happened.
        //
        // It does not manufacture a name where the product wrote none. The
        // three candidates burned on 2026-09-01 (`brama` 0.2.49, 0.2.50 and
        // 0.2.51) wrote no failure line anywhere in their logs -- they stop
        // after `issuing runtime capabilities` and say nothing -- and they stay
        // unclassified when the whole file is read. That is a gap in what the
        // product reports about itself, and this classifier must not paper over
        // it with the nearest-looking label.
        let classified = release_cause::classify(&format!(
            "{why}\n{}\n{}",
            stderr.body.trim_end(),
            stdout.body.trim_end()
        ));
        state.quarantined.insert(
            process.artifact_sha256.clone(),
            QuarantineRecord::classified(reason.clone(), classified),
        );
        state.candidate = None;
        state.phase = RolloutPhase::Quarantined;
        state.detail = reason;
        save_state(target, &mut state)?;
        return Ok(state);
    }

    state.previous = state.active.take();
    state.active = state.candidate.take();
    state.phase = RolloutPhase::Ready;
    state.detail = "candidate readiness passed; stable cutover pending".to_string();
    state.cutover_at = Some(Utc::now());
    save_state(target, &mut state)?;

    let proxy_result = ensure_active_proxy(
        target,
        &serving,
        product,
        desired.rollout_generation,
        &process,
        &mut state,
    )
    .await;
    if let Err(reason) = proxy_result {
        if policy.strategy.automatic_rollback {
            rollback(target, &mut state, reason).await?;
        } else {
            state.phase = RolloutPhase::Failed;
            state.detail = reason;
            save_state(target, &mut state)?;
        }
        return Ok(state);
    }

    state.phase = RolloutPhase::Routed;
    state.detail = format!("stable proxy routed to candidate port {port}");
    save_state(target, &mut state)?;
    tokio::time::sleep(Duration::from_secs(policy.strategy.drain_timeout_seconds)).await;
    let active = state
        .active
        .clone()
        .ok_or_else(|| "routed release lost its active process record".to_string())?;
    if !ready(&active, &serving.readiness_path).await {
        if policy.strategy.automatic_rollback {
            rollback(
                target,
                &mut state,
                "candidate failed during drain".to_string(),
            )
            .await?;
        } else {
            state.phase = RolloutPhase::Failed;
            state.detail =
                "candidate failed during drain; automatic rollback is disabled".to_string();
            save_state(target, &mut state)?;
        }
        return Ok(state);
    }
    state.phase = RolloutPhase::Monitoring;
    state.detail = "previous release drained and retained for rollback window".to_string();
    save_state(target, &mut state)?;
    Ok(state)
}

pub async fn reconcile_once(
    target_name: &str,
    product_filter: Option<&str>,
) -> Result<Vec<HostReleaseState>, String> {
    let document = crate::cli::resolver::canonical_document_or_last_good(target_name)
        .await
        .map_err(|error| error.to_string())?;
    crate::release_control::validate_registry_contract(&document)?;
    // No `release_control` is zero rollout products, NOT the end of the tick.
    //
    // The unit-image revisit policy is a top-level registry key and names its
    // own state directory, so it is entirely independent of whether this
    // document declares any blue-green rollout — and the units it exists for
    // are precisely the ones no rollout owns: the Stado release's own janitor
    // and resolver, and a stream writer this catalogue does not carry.
    // Returning here would have made the feature unreachable on exactly the
    // hosts it was built for.
    let control = crate::release_control::control(&document)?;
    let mut states = Vec::new();
    for (product, policy) in control
        .as_ref()
        .map(|control| control.products.iter())
        .unwrap_or_default()
    {
        if product_filter.is_some_and(|selected| selected != product) {
            continue;
        }
        if policy.strategy.kind != StrategyKind::BlueGreen {
            // A `replace` policy is delivered by the host-release path: the
            // artefact tree is swapped in place, and there is no stable
            // proxy bind or candidate port pair for this reconciler to
            // switch between. The agent must not drive it.
            continue;
        }
        let Some(target) = policy.targets.get(target_name) else {
            continue;
        };
        let Some(_reconcile_lock) = acquire_product_reconcile_lock(target, product)? else {
            continue;
        };
        let control = control
            .as_ref()
            .ok_or_else(|| "release-control product resolved without its document".to_string())?;
        let result = reconcile_product(control, product, policy, target_name, target).await;
        let mut state = match result {
            Ok(state) => state,
            Err(reason) => {
                let mut state = load_state(target, product, target_name)?;
                state.phase = RolloutPhase::Failed;
                state.detail = reason;
                save_state(target, &mut state)?;
                state
            }
        };
        if let Err(error) = publish_status(&state).await {
            state.detail = format!("{}; status publish failed: {error}", state.detail);
            save_state(target, &mut state)?;
        }
        states.push(state);
    }
    // The revisit pass, after the rollouts and never instead of them.
    //
    // A tick's first duty is the release it was asked to deliver; putting a
    // unit back on a file it already declares is repair work, and a repair
    // that delayed a rollout by up to the settle window every tick would be
    // paying for this feature out of the one this agent exists for.
    //
    // It takes the document THIS tick already resolved. A second registry
    // read would be a behavioural change on a fleet that opted into nothing,
    // and the absent-by-default bound promises exactly that it is not one:
    // `release_unit_image::policy` returns `Ok(None)` whenever the document
    // carries no `release_unit_image_revisit` block, before a process table,
    // a unit file or a disk is read.
    //
    // A failure here is reported and never returned. The revisit pass is not
    // the rollout, and a malformed policy or an unreadable ledger must not
    // become a `Failed` phase on a product whose candidate is serving
    // perfectly well. It must not be silent either: a policy block that will
    // not parse, or a contract that will not resolve, is the reason no unit is
    // being repaired, so it is said here — and `registry doctor` reports the
    // same document through `build-refuses-registry`, because the validator
    // that refuses it is wired into `validate_registry_body`.
    let revisit = match crate::release_unit_image::validate_registry_contract(&document) {
        Ok(()) => match crate::release_unit_image::policy(&document) {
            Ok(Some(policy)) => {
                crate::release_unit_image::revisit_once(
                    &document,
                    &policy,
                    target_name,
                    product_filter,
                )
                .await
            }
            Ok(None) => Ok(None),
            Err(reason) => Err(reason),
        },
        Err(reason) => Err(reason),
    };
    match revisit {
        Ok(Some(report)) => eprintln!("{}", report.line()),
        Ok(None) => {}
        Err(reason) => eprintln!(
            "stado release agent unit-image revisit host={target_name} could not run: {reason}"
        ),
    }
    Ok(states)
}

pub async fn agent(
    target_name: &str,
    product_filter: Option<&str>,
    once: bool,
    interval_seconds: u64,
) -> Result<(), String> {
    loop {
        let states = reconcile_once(target_name, product_filter).await?;
        for state in states {
            eprintln!(
                "stado release agent product={} target={} generation={} phase={:?} detail={}",
                state.product, state.target, state.rollout_generation, state.phase, state.detail
            );
        }
        if once {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_secs(interval_seconds.max(5))).await;
    }
}

pub async fn proxy(state_path: &Path, bind: &str) -> Result<(), String> {
    let bind: SocketAddr = bind
        .parse()
        .map_err(|_| "release proxy bind is not a socket address".to_string())?;
    if !bind.ip().is_loopback() {
        return Err("release proxy bind must be loopback".to_string());
    }
    let listener = TcpListener::bind(bind)
        .await
        .map_err(|error| format!("cannot bind release proxy {bind}: {error}"))?;
    loop {
        let (mut client, _) = listener
            .accept()
            .await
            .map_err(|error| format!("release proxy accept failed: {error}"))?;
        let state_path = state_path.to_path_buf();
        tokio::spawn(async move {
            let result = async {
                let state: ProxyState = serde_json::from_slice(
                    &tokio::fs::read(&state_path)
                        .await
                        .map_err(|error| format!("cannot read proxy state: {error}"))?,
                )
                .map_err(|error| format!("invalid proxy state: {error}"))?;
                let upstream: SocketAddr = state
                    .upstream
                    .parse()
                    .map_err(|_| "proxy upstream is not a socket address".to_string())?;
                if !upstream.ip().is_loopback() {
                    return Err("proxy upstream must be loopback".to_string());
                }
                let mut server = TcpStream::connect(upstream)
                    .await
                    .map_err(|error| format!("proxy upstream connect failed: {error}"))?;
                copy_bidirectional(&mut client, &mut server)
                    .await
                    .map_err(|error| format!("release proxy failed: {error}"))?;
                Ok::<(), String>(())
            }
            .await;
            if let Err(error) = result {
                eprintln!("stado release proxy connection failed: {error}");
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state_with(quarantines: &[(&str, QuarantineCause, &str)]) -> HostReleaseState {
        let mut state = HostReleaseState::new("brama", "charless-mac-mini");
        for (index, (digest, cause, stamp)) in quarantines.iter().enumerate() {
            state.quarantined.insert(
                (*digest).to_string(),
                QuarantineRecord {
                    reason: format!("candidate did not become ready within 90s: reason {index}"),
                    quarantined_at: DateTime::parse_from_rfc3339(stamp)
                        .expect("fixture stamp parses")
                        .with_timezone(&Utc),
                    cause: *cause,
                    evidence: format!("evidence {index}"),
                },
            );
        }
        state
    }

    /// The digests and stamps are the live `brama` rows from
    /// `charless-mac-mini` for 0.2.49, 0.2.50 and 0.2.51 — the three candidates
    /// burned inside five hours on 2026-09-01, at the interval this rule is
    /// meant for.
    ///
    /// The cause is supplied. On the real host those three wrote no failure
    /// line anywhere in their logs and classify as `unclassified`, so no
    /// refusal arms there and none should. The run is constructed because the
    /// rule has to be exercised somewhere, and inventing the timestamps too
    /// would have hidden that the real sequence is this tight.
    const RUN: &[(&str, QuarantineCause, &str)] = &[
        (
            "217167ef",
            QuarantineCause::CredentialCannotServe,
            "2026-09-01T10:34:46Z",
        ),
        (
            "d862fb1b",
            QuarantineCause::CredentialCannotServe,
            "2026-09-01T10:54:23Z",
        ),
        (
            "4c2bb7c3",
            QuarantineCause::CredentialCannotServe,
            "2026-09-01T15:40:50Z",
        ),
    ];

    /// Build the hold the same way `cause_hold` does, but from a supplied
    /// verdict instead of a spawned process.
    ///
    /// The decision and the spawn are separated so the decision can be
    /// exercised for every verdict, including the ones a real host will not
    /// produce on demand — a vault that will not open, a timeout. The spawn
    /// itself is [`ask_wall`] and is the part that cannot be tested without a
    /// host; what it returns is exactly this enum.
    fn decide(
        state: &HostReleaseState,
        verdict: Option<(release_cause::WallVerdict, Option<String>)>,
    ) -> Option<CauseHold> {
        let run = cause_run(state)?;
        let repeated = |unreachable: Option<String>| {
            run.repeats().then(|| CauseHold {
                cause: run.cause,
                evidence: run.evidence.clone(),
                digests: run.digests.clone(),
                ground: HoldGround::Repeated {
                    count: run.len(),
                    since: run.since,
                    unreachable,
                },
            })
        };
        match verdict {
            None => repeated(None),
            Some((release_cause::WallVerdict::Present, detail)) => Some(CauseHold {
                cause: run.cause,
                evidence: run.evidence.clone(),
                digests: run.digests.clone(),
                ground: HoldGround::Observed {
                    check: "skarbiec routes verify provider:kimi".to_string(),
                    detail: detail.unwrap_or_else(|| run.evidence.clone()),
                    at: Utc::now(),
                },
            }),
            Some((release_cause::WallVerdict::Gone, _)) => None,
            Some((release_cause::WallVerdict::Unknown, why)) => repeated(why),
        }
    }

    const PRESENT: Option<(release_cause::WallVerdict, Option<String>)> =
        Some((release_cause::WallVerdict::Present, None));
    const GONE: Option<(release_cause::WallVerdict, Option<String>)> =
        Some((release_cause::WallVerdict::Gone, None));

    #[test]
    fn an_observed_wall_holds_at_the_very_first_quarantine() {
        // This is the whole point of the change. One row plus a condition that
        // still reports the wall is sufficient; the old rule spent two more
        // candidates establishing what the check already said.
        let hold = decide(&state_with(&RUN[..1]), PRESENT)
            .expect("one quarantine and an observed wall must hold");
        assert!(matches!(hold.ground, HoldGround::Observed { .. }));
        let sentence = hold.sentence();
        assert!(
            sentence.contains("still refuses") && sentence.contains("Checked with"),
            "an observed hold must say what it observed and when: {sentence}"
        );
    }

    #[test]
    fn a_repaired_credential_releases_promotion_with_no_override() {
        // The property counting cannot have. The operator refills the vault
        // field, the check stops failing, and the next candidate goes -- no
        // `quarantine clear`, nothing to remember.
        assert!(decide(&state_with(&RUN[..1]), GONE).is_none());
        // Including past the counting limit: an observation beats a tally.
        assert!(decide(&state_with(RUN), GONE).is_none());
    }

    #[test]
    fn an_unreachable_check_never_releases_a_hold_and_never_invents_one() {
        let unknown =
            |why: &str| Some((release_cause::WallVerdict::Unknown, Some(why.to_string())));
        // Not permission to promote: the count still governs, exactly as before
        // the predicate existed.
        let hold = decide(&state_with(RUN), unknown("no skarbiec binary at /x"))
            .expect("an unreachable check must fall back to counting, not release");
        match &hold.ground {
            HoldGround::Repeated { unreachable, .. } => assert_eq!(
                unreachable.as_deref(),
                Some("no skarbiec binary at /x"),
                "the refusal must admit the check could not be reached"
            ),
            other => panic!("expected a counted hold, got {other:?}"),
        }
        assert!(
            hold.sentence().contains("could not be checked"),
            "the ground must not read as an observation: {}",
            hold.sentence()
        );
        // And it does not manufacture a refusal on a short run either.
        assert!(decide(&state_with(&RUN[..1]), unknown("timeout")).is_none());
    }

    #[test]
    fn counting_still_governs_a_cause_with_no_condition_to_ask() {
        // N=3 unchanged where it applies. `verdict: None` is what `cause_hold`
        // does when the cause has no predicate.
        let rows: Vec<(&str, QuarantineCause, &str)> = RUN
            .iter()
            .map(|(digest, _, stamp)| {
                (
                    *digest,
                    QuarantineCause::CapabilityRedemptionRefused,
                    *stamp,
                )
            })
            .collect();
        assert!(decide(&state_with(&rows[..2]), None).is_none());
        let hold = decide(&state_with(&rows), None).expect("three of one cause still holds");
        assert!(matches!(hold.ground, HoldGround::Repeated { .. }));
    }

    #[test]
    fn a_run_of_unnamed_causes_never_holds_anything() {
        // Twelve of the twenty live records are unclassified, seven of them
        // consecutively. Refusing on a cause the agent could not name would
        // have frozen this product for a month on no evidence at all.
        let unnamed: Vec<(&str, QuarantineCause, &str)> = RUN
            .iter()
            .map(|(digest, _, stamp)| (*digest, QuarantineCause::Unclassified, *stamp))
            .collect();
        assert!(cause_run(&state_with(&unnamed)).is_none());
        assert!(decide(&state_with(&unnamed), PRESENT).is_none());
    }

    #[test]
    fn one_different_cause_below_the_top_shortens_the_run() {
        // The live shape: b54ea076 credential_cannot_serve sits above
        // aba3c3b2 rollback_compatibility_undeclared, so the run is ONE.
        // Counting alone would not hold, which is why the condition matters.
        let mut mixed = RUN.to_vec();
        mixed[1].1 = QuarantineCause::RollbackCompatibilityUndeclared;
        let run = cause_run(&state_with(&mixed)).expect("the newest row still names a cause");
        assert_eq!(run.cause, QuarantineCause::CredentialCannotServe);
        assert_eq!(run.len(), 1);
        assert!(!run.repeats());
        // Counting lets it burn; the observed wall does not.
        assert!(decide(&state_with(&mixed), None).is_none());
        assert!(decide(&state_with(&mixed), PRESENT).is_some());
    }

    #[test]
    fn recency_is_read_from_the_stamp_not_from_the_digest_order() {
        // The map is keyed by digest, so its iteration order is alphabetical.
        // A rule that trusted that order would pick the wrong rows.
        let mut rows = RUN.to_vec();
        rows.push((
            "0000aaaa",
            QuarantineCause::RollbackCompatibilityUndeclared,
            "2026-08-06T15:49:52Z",
        ));
        let run = cause_run(&state_with(&rows))
            .expect("the oldest row sorts first by digest and must not join the run");
        assert_eq!(run.cause, QuarantineCause::CredentialCannotServe);
        assert_eq!(run.len(), REPEAT_CAUSE_LIMIT);
        assert!(!run.digests.iter().any(|digest| digest == "0000aaaa"));
        // The oldest member of the run, not of the map.
        assert_eq!(run.since.to_rfc3339(), "2026-09-01T10:34:46+00:00");
    }

    #[test]
    fn every_refusal_names_a_way_out() {
        for verdict in [PRESENT, None] {
            let sentence = decide(&state_with(RUN), verdict)
                .expect("a run of three holds either way")
                .sentence();
            assert!(
                sentence.contains("stado release quarantine clear --digest 217167ef"),
                "refusal must name the existing override and a real digest: {sentence}"
            );
            assert!(
                sentence.contains("skarbiec routes verify"),
                "refusal must carry the cause's remedy: {sentence}"
            );
        }
    }

    /// Retention: the clip used to keep the head and drop the end, and both
    /// ends carry decisive lines in the live records.
    #[test]
    fn a_clipped_tail_keeps_both_of_its_ends() {
        let text = format!("HEAD-MARKER{}TAIL-MARKER", "x".repeat(4000));
        let clipped = clip_middle(&text, 200);
        assert!(clipped.starts_with("HEAD-MARKER"), "{clipped}");
        assert!(clipped.ends_with("TAIL-MARKER"), "{clipped}");
        assert!(
            clipped.contains("elided"),
            "an elision must say how much it dropped: {clipped}"
        );
        assert_eq!(clip_middle("short", 200), "short");
    }

    #[test]
    fn a_quarantine_record_names_its_cause_from_its_reason() {
        let record = QuarantineRecord::new(
            "release 0.2.54 does not declare rollback compatibility with 0.2.53".to_string(),
        );
        assert_eq!(
            record.cause,
            QuarantineCause::RollbackCompatibilityUndeclared
        );
        assert!(!record.evidence.is_empty());
    }

    /// The twenty records already on the live host carry neither field, and
    /// this struct refuses unknown fields. Both directions have to work.
    #[test]
    fn a_record_written_before_this_change_still_parses() {
        let legacy = r#"{"reason":"candidate did not become ready before deadline",
            "quarantined_at":"2026-08-06T15:49:52.887004+00:00"}"#;
        let record: QuarantineRecord =
            serde_json::from_str(legacy).expect("a legacy record must still parse");
        assert_eq!(record.cause, QuarantineCause::Unclassified);
        assert!(record.evidence.is_empty());
    }
}
