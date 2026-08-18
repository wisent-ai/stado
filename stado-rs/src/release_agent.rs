//! Host-side blue-green release reconciliation.
//!
//! The agent consumes exact desired release coordinates from the canonical
//! registry, verifies signed immutable artifacts, stages them in versioned
//! directories, starts a candidate on an internal port, and atomically changes
//! a stable loopback proxy. The previous process stays warm for the rollback
//! window. Failed digests are quarantined and cannot loop indefinitely.

use std::collections::BTreeMap;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::net::SocketAddr;
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QuarantineRecord {
    pub reason: String,
    pub quarantined_at: DateTime<Utc>,
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

fn state_path(target: &ReleaseTargetPolicy, product: &str) -> PathBuf {
    Path::new(&target.state_dir).join(format!("{product}.json"))
}

fn proxy_state_path(target: &ReleaseTargetPolicy, product: &str) -> PathBuf {
    Path::new(&target.state_dir).join(format!("{product}-proxy.json"))
}

fn atomic_json<T: Serialize>(path: &Path, value: &T) -> Result<(), String> {
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
    let bytes = serde_json::to_vec(value)
        .map_err(|error| format!("cannot encode release state: {error}"))?;
    {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&staging)
            .map_err(|error| {
                format!("cannot create state staging {}: {error}", staging.display())
            })?;
        file.write_all(&bytes)
            .and_then(|()| file.write_all(b"\n"))
            .and_then(|()| file.sync_all())
            .map_err(|error| {
                format!("cannot write state staging {}: {error}", staging.display())
            })?;
    }
    std::fs::rename(&staging, path)
        .map_err(|error| format!("cannot commit release state {}: {error}", path.display()))
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
    let state: HostReleaseState = serde_json::from_slice(
        &std::fs::read(&path)
            .map_err(|error| format!("cannot read release state {}: {error}", path.display()))?,
    )
    .map_err(|error| format!("invalid release state {}: {error}", path.display()))?;
    if state.schema_version != STATE_SCHEMA
        || state.product != product
        || state.target != target_name
    {
        return Err(format!(
            "release state identity mismatch at {}",
            path.display()
        ));
    }
    Ok(state)
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

fn release_log(
    target: &ReleaseTargetPolicy,
    product: &str,
    version: &str,
    stream: &str,
) -> Result<File, String> {
    std::fs::create_dir_all(&target.logs_root)
        .map_err(|error| format!("cannot create release logs {}: {error}", target.logs_root))?;
    let path = Path::new(&target.logs_root).join(format!("{product}-{version}.{stream}"));
    OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .map_err(|error| format!("cannot open release log {}: {error}", path.display()))
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
    let mut last = None;
    loop {
        match not_ready_because(record, readiness_path).await {
            None => return None,
            Some(reason) => last = Some(reason),
        }
        if tokio::time::Instant::now() >= deadline {
            return last;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}

/// Does something already answer the stable bind's readiness path?
///
/// The agent tracks its proxy by pid, so a proxy left behind by an earlier run is
/// invisible to it: `proxy_alive` says no, the spawn fails with `Address already
/// in use`, and the rollout is failed for a bind that is being served correctly.
/// Asking the port rather than the record is the only way to tell a stale pid from
/// a dead proxy.
/// Returns what the bind answered, so a declined adoption records evidence rather
/// than a silent `false`. The first version returned a bool, declined once, and
/// left no way to tell a refused connection from a proxy pointing at an upstream
/// that had just been terminated.
async fn stable_bind_answer(serving: &BlueGreenServing) -> Result<(), String> {
    let url = format!(
        "http://{}{}",
        serving.stable_bind, serving.readiness_path
    );
    match reqwest::Client::new()
        .get(&url)
        .timeout(Duration::from_secs(3))
        .send()
        .await
    {
        Ok(response) if response.status().is_success() => Ok(()),
        Ok(response) => Err(format!("{url} answered HTTP {}", response.status())),
        Err(error) if error.is_timeout() => Err(format!("{url} did not answer within 3s")),
        Err(error) if error.is_connect() => Err(format!("{url} refused the connection")),
        Err(error) => Err(format!("{url} failed: {error}")),
    }
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
    if status.success() || status.code() == Some(3) || status.code() == Some(5) {
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

fn proxy_alive(state: &HostReleaseState) -> bool {
    state.proxy_pid.is_some_and(pid_alive)
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

async fn fetch_release_bytes(uri: &str) -> Result<Vec<u8>, String> {
    // The release channel is served publicly over the object API.
    // Without STADO_API_URL, JobStorage::read_bytes on the canonical root
    // prefix serves a stale copy and the agent quarantines the release.
    crate::cli::storage::fetch_object(uri)
        .await
        .map_err(|e| e.to_string())
}

async fn fetch_candidate(
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

/// Every process running out of this product's `releases/` directory:
/// `(pid, version, port)`, the port parsed from a `--port N` argument when the
/// launcher passed one.
///
/// These processes are all children of some run of this agent -- nothing else
/// executes from that directory -- so any of them the state file does not name is
/// a leak from a run that died between spawning and recording.
fn release_processes(install_root: &str) -> Vec<(i32, String, Option<u16>)> {
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
        let version = line
            .split(&marker)
            .nth(1)
            .and_then(|tail| tail.split('/').next())
            .unwrap_or("?")
            .to_string();
        let arguments: Vec<&str> = fields.collect();
        let mut port = None;
        for pair in arguments.windows(2) {
            if pair[0] == "--port" {
                port = pair[1].parse().ok();
            }
        }
        found.push((pid, version, port));
    }
    found
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
    for record in [&state.active, &state.candidate, &state.previous] {
        if let Some(record) = record {
            known.push(record.pid);
        }
    }
    if let Some(pid) = state.proxy_pid {
        known.push(pid);
    }
    let upstream = proxy_upstream_port(target, product);
    for (pid, version, port) in release_processes(install_root) {
        if known.contains(&pid) {
            continue;
        }
        if upstream.is_some() && port == upstream {
            eprintln!(
                "leaked {product} {version} pid={pid} still carries traffic on \
                 {port:?}; retiring by cutover, not by kill"
            );
            continue;
        }
        let _ = kill(Pid::from_raw(pid), Signal::SIGTERM);
        eprintln!("swept leaked {product} {version} pid={pid} port={port:?}");
    }
}

fn next_port(
    candidate_ports: [u16; 2],
    state: &HostReleaseState,
    occupied: Option<u16>,
) -> u16 {
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

async fn rollback(
    target: &ReleaseTargetPolicy,
    state: &mut HostReleaseState,
    reason: String,
) -> Result<(), String> {
    let failed = state.active.take().or_else(|| state.candidate.take());
    if let Some(previous) = state.previous.take() {
        if proxy_alive(state) {
            write_proxy_target(
                target,
                &state.product,
                state.rollout_generation,
                previous.port,
            )?;
        } else {
            let serving = target.blue_green_serving()?;
            state.proxy_pid = Some(start_proxy(
                target,
                &serving,
                &state.product,
                state.rollout_generation,
                previous.port,
            )?);
            tokio::time::sleep(Duration::from_millis(200)).await;
            if !proxy_alive(state) {
                return Err("rollback proxy failed to start".to_string());
            }
        }
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

async fn reconcile_product(
    control: &ReleaseControl,
    product: &str,
    policy: &ProductReleasePolicy,
    target_name: &str,
    target: &ReleaseTargetPolicy,
) -> Result<HostReleaseState, String> {
    let mut state = load_state(target, product, target_name)?;
    // `reconcile_once` hands only blue-green policies to this function; ask
    // for the serving coordinates by name rather than re-checking the
    // validator's invariant, so a replace policy reaching here fails loudly
    // instead of halfway through a rollout.
    let serving = target.blue_green_serving()?;
    // Reconcile the process world before reasoning from the record: anything
    // running out of this product's releases directory that the record does not
    // name is a leak from a run that died between spawning and saving.
    sweep_leaked_processes(target, product, &policy.install_root, &state);
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
    state.rollout_generation = desired.rollout_generation;

    if state.quarantined.contains_key(&artifact.artifact_sha256) {
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
        if !ready(&active, &serving.readiness_path).await {
            if policy.strategy.automatic_rollback {
                rollback(
                    target,
                    &mut state,
                    "active release lost readiness".to_string(),
                )
                .await?;
            } else {
                state.phase = RolloutPhase::Failed;
                state.detail =
                    "active release lost readiness; automatic rollback is disabled".to_string();
                save_state(target, &mut state)?;
            }
            return Ok(state);
        }
        let proxy_result = if proxy_alive(&state) {
            write_proxy_target(target, product, desired.rollout_generation, active.port)
        } else {
            stop_legacy(target)?;
            state.proxy_pid = Some(start_proxy(
                target,
                &serving,
                product,
                desired.rollout_generation,
                active.port,
            )?);
            tokio::time::sleep(Duration::from_millis(200)).await;
            if proxy_alive(&state) {
                Ok(())
            } else {
                match stable_bind_answer(&serving).await {
                    Ok(()) => {
                        state.proxy_pid = None;
                        state.detail =
                            format!("adopted the proxy already serving {}", serving.stable_bind);
                        Ok(())
                    }
                    Err(why) => Err(format!("stable release proxy failed to start: {why}")),
                }
            }
        };
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
        if matches!(state.phase, RolloutPhase::Ready | RolloutPhase::Routed) {
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
                    QuarantineRecord {
                        reason: reason.clone(),
                        quarantined_at: Utc::now(),
                    },
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
                QuarantineRecord {
                    reason: reason.clone(),
                    quarantined_at: Utc::now(),
                },
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
    if let Some(why) =
        await_ready_because(&process, &serving.readiness_path, policy.strategy.readiness_timeout_seconds).await
    {
        terminate(&process);
        let reason = format!(
            "candidate did not become ready within {}s: {why}",
            policy.strategy.readiness_timeout_seconds
        );
        state.quarantined.insert(
            process.artifact_sha256.clone(),
            QuarantineRecord {
                reason: reason.clone(),
                quarantined_at: Utc::now(),
            },
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

    let proxy_result = if proxy_alive(&state) {
        write_proxy_target(target, product, desired.rollout_generation, port)
    } else {
        stop_legacy(target)?;
        state.proxy_pid = Some(start_proxy(
            target,
            &serving,
            product,
            desired.rollout_generation,
            port,
        )?);
        tokio::time::sleep(Duration::from_millis(200)).await;
        if proxy_alive(&state) {
            Ok(())
        } else {
            // Someone may already serve the stable bind: the target file this
            // agent just wrote is what every Stado proxy reads, so a proxy left by
            // an earlier run is already forwarding to this candidate. Spawning a
            // second binder returns `Address already in use (os error 48)`, and the
            // rollout used to fail for a bind that was serving correctly -- state
            // said "no proxy", the port said otherwise, and nothing compared them.
            match stable_bind_answer(&serving).await {
                Ok(()) => {
                    state.proxy_pid = None;
                    state.detail =
                        format!("adopted the proxy already serving {}", serving.stable_bind);
                    Ok(())
                }
                Err(why) => Err(format!("stable release proxy failed to start: {why}")),
            }
        }
    };
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

pub async fn reconcile_once(target_name: &str) -> Result<Vec<HostReleaseState>, String> {
    let document = crate::cli::resolver::canonical_document(target_name)
        .await
        .map_err(|error| error.to_string())?;
    crate::release_control::validate_registry_contract(&document)?;
    let Some(control) = crate::release_control::control(&document)? else {
        return Ok(Vec::new());
    };
    let mut states = Vec::new();
    for (product, policy) in &control.products {
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
        let result = reconcile_product(&control, product, policy, target_name, target).await;
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
    Ok(states)
}

pub async fn agent(target_name: &str, once: bool, interval_seconds: u64) -> Result<(), String> {
    loop {
        let states = reconcile_once(target_name).await?;
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

    #[test]
    fn pid_alive_reaps_an_exited_child() {
        let child = Command::new("/usr/bin/true").spawn().unwrap();
        let pid = child.id() as i32;
        std::thread::sleep(Duration::from_millis(100));
        assert!(!pid_alive(pid));
    }

    #[test]
    fn pid_alive_accepts_a_running_child() {
        let mut child = Command::new("/bin/sleep").arg("5").spawn().unwrap();
        assert!(pid_alive(child.id() as i32));
        child.kill().unwrap();
        child.wait().unwrap();
    }

    #[test]
    fn terminate_signals_the_release_process_group() {
        let temporary = tempfile::tempdir().unwrap();
        let child_pid_path = temporary.path().join("child.pid");
        let shell = Command::new("/bin/sh")
            .args(["-c", "sleep 30 & echo $! > \"$1\"; wait", "release-test"])
            .arg(&child_pid_path)
            .process_group(0)
            .spawn()
            .unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while !child_pid_path.exists() && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        let child_pid: i32 = std::fs::read_to_string(&child_pid_path)
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        let record = ProcessRecord {
            version: "test".to_string(),
            artifact_sha256: "test".to_string(),
            manifest_sha256: "test".to_string(),
            port: 1,
            pid: shell.id() as i32,
            release_dir: "/tmp/test".to_string(),
            started_at: Utc::now(),
        };

        terminate(&record);

        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while (pid_alive(record.pid) || kill(Pid::from_raw(child_pid), None).is_ok())
            && std::time::Instant::now() < deadline
        {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(!pid_alive(record.pid));
        assert!(kill(Pid::from_raw(child_pid), None).is_err());
    }
}
