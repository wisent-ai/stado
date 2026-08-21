//! `stado resolver` — logical service discovery and local data plane.

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::process::Stdio;
use std::sync::Arc;
use std::time::{Duration, Instant};

use clap::Subcommand;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::process::Command;
use tokio::sync::RwLock;
use tokio::task::JoinSet;

use crate::monitor::host_silence;
use crate::service_resolution::{self, ResolvedService, ResolverAdapter, ResolverConfig};
use crate::targets::{self, RegistryStore};

use super::CmdError;

const REQUEST_HEAD_LIMIT: usize = 16 * 1024;

#[derive(Debug, Subcommand)]
pub enum ResolverCommands {
    /// Resolve one logical service for an authorized workload.
    Resolve {
        /// Logical service name, with or without the stado://service/ prefix.
        service: String,
        /// Stable workload identity from the service unit.
        #[arg(long)]
        consumer: String,
        /// Emit machine-readable output.
        #[arg(long)]
        json: bool,
    },
    /// Run the local resolution API and configured stable-port adapters.
    Serve {
        /// Exact registry target whose service_resolver policy to enforce.
        #[arg(long)]
        target: String,
    },
    /// Whether this host's resolver is ready, and why not when it is not.
    ///
    /// A subcommand rather than another endpoint on the resolver's own API,
    /// for one reason: the question is asked when the resolver is DOWN. On
    /// 2026-08-19 this host's resolver sat in a launchd restart loop holding a
    /// dead ssh control socket, and an answer served on `api_bind` would have
    /// been unreachable for exactly the window an operator needed it. This
    /// reads the registry and the state `serve` publishes to
    /// [`state_path`], so it answers with the resolver stopped, and exits
    /// non-zero when the answer is not `ready` so a unit or a script can act
    /// on it. The live process's own `/health` remains where a workload
    /// checks a resolver it is already talking to.
    Status {
        /// Registry target whose resolver to report on. Defaults to the
        /// target the published state names, then to this host's identity.
        #[arg(long)]
        target: Option<String>,
        /// Emit machine-readable output.
        #[arg(long)]
        json: bool,
    },
    /// Emit this host's versioned registry for authenticated resolver peers.
    #[command(hide = true)]
    Snapshot,
}

pub async fn dispatch(command: ResolverCommands) -> Result<(), CmdError> {
    match command {
        ResolverCommands::Resolve {
            service,
            consumer,
            json,
        } => resolve_once(&service, &consumer, json).await,
        ResolverCommands::Serve { target } => serve(&target).await,
        ResolverCommands::Snapshot => emit_snapshot().await,
        ResolverCommands::Status { target, json } => status(target.as_deref(), json).await,
    }
}

fn logical_name(value: &str) -> Result<&str, CmdError> {
    let value = value
        .strip_prefix("stado://service/")
        .unwrap_or(value)
        .trim();
    if value.is_empty() || value.contains('/') {
        return Err(CmdError::usage(
            "SERVICE must be one logical name or stado://service/<name>",
        ));
    }
    Ok(value)
}

const SNAPSHOT_LIMIT: usize = 1024 * 1024;

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SnapshotPayload {
    store_version: String,
    document: Value,
}

fn validate_snapshot(payload: SnapshotPayload) -> Result<(Value, String, u64), String> {
    targets::validate_registry(&payload.document).map_err(|error| error.to_string())?;
    let directory = service_resolution::directory(&payload.document)?
        .ok_or_else(|| "registry.service_directory is required".to_string())?;
    Ok((
        payload.document,
        payload.store_version,
        directory.generation,
    ))
}

pub(crate) async fn read_local_snapshot(store: &RegistryStore) -> Result<(Value, String, u64), String> {
    let blob = store
        .read_versioned()
        .await
        .map_err(|error| format!("registry read failed: {error}"))?
        .ok_or_else(|| format!("no registry document at {}", store.location()))?;
    let document: Value = serde_json::from_str(&blob.content)
        .map_err(|error| format!("invalid registry JSON: {error}"))?;
    validate_snapshot(SnapshotPayload {
        store_version: blob.version,
        document,
    })
}

fn ssh_command() -> Command {
    let mut command = Command::new("ssh");
    command.args([
        "-T",
        "-F",
        "/dev/null",
        "-o",
        "BatchMode=yes",
        "-o",
        "StrictHostKeyChecking=yes",
        "-o",
        "ConnectTimeout=10",
        "-o",
        "ServerAliveInterval=10",
        "-o",
        "ServerAliveCountMax=2",
        "-o",
        "ControlMaster=auto",
        "-o",
        "ControlPersist=60",
    ]);
    if let Ok(home) = std::env::var("HOME") {
        command
            .arg("-o")
            .arg(format!("ControlPath={home}/.stado/resolver-ssh-%C"));
    }
    let key_file = std::env::var("STADO_RESOLVER_SSH_KEY_FILE")
        .ok()
        .filter(|path| !path.trim().is_empty())
        .map(std::path::PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME")
                .map(std::path::PathBuf::from)
                .map(|home| home.join(".stado").join("resolver-ssh-key"))
                .filter(|path| path.is_file())
        });
    if let Some(key_file) = key_file {
        command
            .args(["-o", "IdentitiesOnly=yes", "-i"])
            .arg(key_file);
    }
    command
}

/// Publish one `authority_unreachable` refusal about the authority host.
///
/// The evidence belongs to the AUTHORITY, not to the machine that noticed.
/// When the Mac mini dropped off the tailnet on 2026-08-19 this read failed
/// on the laptop with "registry authority exited with ...: ssh: connect to
/// host ... Operation timed out", and that sentence was the clearest
/// statement anything in the fleet made about the Mac mini being gone. It
/// went to `~/.stado/logs/stado-resolver.err` and nowhere else. It now also
/// lands in `reader_refusals/<authority>/`, where `stado host link
/// <authority>` will find it — verbatim, because a rephrased sentence is a
/// second vocabulary for one condition and sends an operator grepping for a
/// string that exists in no source file.
///
/// Best effort and bounded by [`host_silence::report_refusal`]: this runs
/// inside a failing read and must never replace that read's own error.
async fn refuse_authority(target: &str, reader: &str, sentence: &str) {
    host_silence::report_refusal(
        target,
        reader,
        host_silence::REASON_AUTHORITY_UNREACHABLE,
        sentence,
    )
    .await;
}

#[derive(Clone)]
pub(crate) enum SnapshotSource {
    Local(Arc<RegistryStore>),
    Authority {
        /// Registry name of the authority target, carried so a failed read
        /// can name the host that is actually silent.
        target: String,
        ssh: String,
        command: String,
    },
}

impl SnapshotSource {
    /// The host a failure of this source is evidence about.
    fn subject_host(&self, local_target: &str) -> String {
        match self {
            Self::Local(_) => local_target.to_string(),
            Self::Authority { target, .. } => target.clone(),
        }
    }

    /// `reader` is the refusal vocabulary's word for who is reading:
    /// [`host_silence::READER_RESOLVER`] for the serving loop and its
    /// background refresh, [`host_silence::READER_CLI`] for a one-shot
    /// command.
    pub(crate) async fn fetch(&self, reader: &str) -> Result<(Value, String, u64), String> {
        match self {
            Self::Local(store) => read_local_snapshot(store).await,
            // Unconditional on every failed authority read, not only on
            // startup. A control master outlives the process that opened it by
            // `ControlPersist`, and one whose connection has already died
            // answers nothing while looking perfectly alive: this resolver
            // spent 2026-08-19 in a launchd restart loop reading the registry
            // through a socket that could not succeed again, and only repeated
            // `launchctl kickstart` cleared it. Dropping it costs one TCP
            // connect and one authentication on the next attempt; keeping it
            // costs every attempt for as long as the process lives.
            Self::Authority {
                target,
                ssh,
                command,
            } => Self::fetch_authority(target, ssh, command, reader)
                .await
                .map_err(|error| {
                    drop_stale_ssh_sockets();
                    error
                }),
        }
    }

    async fn fetch_authority(
        target: &str,
        ssh: &str,
        command: &str,
        reader: &str,
    ) -> Result<(Value, String, u64), String> {
        let remote_command = format!("{} resolver snapshot", crate::deploy::shlex_quote(command));
        let output = match ssh_command()
            .arg(ssh)
            .arg(remote_command)
            .stderr(Stdio::piped())
            .output()
            .await
        {
            Ok(output) => output,
            Err(error) => {
                let sentence = format!("registry authority SSH failed: {error}");
                refuse_authority(target, reader, &sentence).await;
                return Err(sentence);
            }
        };
        if !output.status.success() {
            let detail = String::from_utf8_lossy(&output.stderr);
            let detail = detail.trim();
            let sentence = if detail.is_empty() {
                format!("registry authority exited with {}", output.status)
            } else {
                format!(
                    "registry authority exited with {}: {}",
                    output.status,
                    detail.chars().take(4096).collect::<String>()
                )
            };
            // Only the two transport branches publish. An authority that
            // answers with an oversized or unparseable snapshot is reachable
            // and wrong, which is a different finding from a silent host and
            // must not be counted as one.
            refuse_authority(target, reader, &sentence).await;
            return Err(sentence);
        }
        if output.stdout.len() > SNAPSHOT_LIMIT {
            return Err("registry authority snapshot exceeds 1 MiB".to_string());
        }
        let payload: SnapshotPayload = serde_json::from_slice(&output.stdout)
            .map_err(|error| format!("invalid registry authority response: {error}"))?;
        validate_snapshot(payload)
    }
}

fn parsed_registry(document: &Value) -> Result<targets::Registry, String> {
    targets::load_registry_from_str(
        &serde_json::to_string(document).map_err(|error| error.to_string())?,
    )
    .map_err(|error| error.to_string())
}

pub(crate) fn current_target(document: &Value) -> Result<String, String> {
    let hostname = crate::providers::vast::system_hostname();
    parsed_registry(document)?
        .lookup_self(&hostname)
        .map_err(|error| error.to_string())?
        .map(|target| target.name.clone())
        .ok_or_else(|| format!("resolver host {hostname:?} has no registry target identity"))
}

pub(crate) fn snapshot_source(
    local_store: Arc<RegistryStore>,
    document: &Value,
    local_target: &str,
) -> Result<SnapshotSource, String> {
    let directory = service_resolution::directory(document)?
        .ok_or_else(|| "registry.service_directory is required".to_string())?;
    if directory.authority.target == local_target {
        return Ok(SnapshotSource::Local(local_store));
    }
    let registry = parsed_registry(document)?;
    let target = registry
        .lookup(&directory.authority.target)
        .ok_or_else(|| "registry authority target disappeared".to_string())?;
    let ssh = target
        .ssh
        .clone()
        .ok_or_else(|| "registry authority has no SSH transport".to_string())?;
    Ok(SnapshotSource::Authority {
        target: target.name.clone(),
        ssh,
        command: directory.authority.command,
    })
}

/// Fetch the canonical registry document directly from the configured Stado
/// registry store. Release agents never depend on an SSH hop through the
/// service-directory authority.
pub async fn canonical_document(local_target: &str) -> Result<Value, CmdError> {
    let (document, _) = super::registry::fetch_versioned_document().await?;
    let detected = current_target(&document).map_err(CmdError::click)?;
    if detected != local_target {
        return Err(CmdError::click(format!(
            "release agent target {local_target:?} does not match this host {detected:?}"
        )));
    }
    Ok(document)
}

async fn emit_snapshot() -> Result<(), CmdError> {
    let store = RegistryStore::open().await?;
    let (document, store_version, _) =
        read_local_snapshot(&store).await.map_err(CmdError::click)?;
    println!(
        "{}",
        serde_json::to_string(&SnapshotPayload {
            store_version,
            document,
        })?
    );
    Ok(())
}

async fn resolve_once(service: &str, consumer: &str, json_output: bool) -> Result<(), CmdError> {
    let service = logical_name(service)?;
    let store = Arc::new(RegistryStore::open().await?);
    let (bootstrap, _, _) = read_local_snapshot(&store).await.map_err(CmdError::click)?;
    let target = current_target(&bootstrap).map_err(CmdError::click)?;
    let source = snapshot_source(store, &bootstrap, &target).map_err(CmdError::click)?;
    let (document, _, _) = source
        .fetch(host_silence::READER_CLI)
        .await
        .map_err(CmdError::click)?;
    let resolved =
        service_resolution::resolve(&document, service, consumer).map_err(CmdError::click)?;
    let report = json!({
        "service": format!("stado://service/{}", resolved.name),
        "generation": resolved.generation,
        "capabilities": resolved.capabilities,
    });
    if json_output {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!(
            "{} generation={} capabilities={}",
            report["service"].as_str().unwrap_or_default(),
            resolved.generation,
            resolved.capabilities.join(",")
        );
    }
    Ok(())
}

struct Snapshot {
    document: Value,
    store_version: String,
    directory_generation: u64,
    loaded_at: Instant,
    /// The same instant as a wall clock, because [`PublishedState`] is read by
    /// another process and a monotonic `Instant` means nothing there.
    loaded_at_iso: String,
}

struct ResolverState {
    local_store: Arc<RegistryStore>,
    source: RwLock<SnapshotSource>,
    snapshot: RwLock<Snapshot>,
    max_stale: Duration,
    local_target: String,
    adapters: Vec<ResolverAdapter>,
    config: ResolverConfig,
}

impl ResolverState {
    async fn refresh(&self) -> Result<bool, String> {
        let source = self.source.read().await.clone();
        let (document, store_version, generation) =
            source.fetch(host_silence::READER_RESOLVER).await?;
        let next_source =
            snapshot_source(Arc::clone(&self.local_store), &document, &self.local_target)?;
        let next_config = service_resolution::resolver_config(&document, &self.local_target)?;
        if next_config != self.config {
            return Ok(true);
        }
        let mut current = self.snapshot.write().await;
        if generation < current.directory_generation {
            return Err(format!(
                "service directory rollback rejected: generation {generation} < {}",
                current.directory_generation
            ));
        }
        if generation == current.directory_generation
            && service_resolution::directory(&document)?
                != service_resolution::directory(&current.document)?
        {
            return Err(format!(
                "service directory changed without advancing generation {generation}"
            ));
        }
        current.document = document;
        current.store_version = store_version;
        current.directory_generation = generation;
        current.loaded_at = Instant::now();
        current.loaded_at_iso = now_iso();
        drop(current);
        *self.source.write().await = next_source;
        Ok(false)
    }

    async fn resolve(&self, service: &str, consumer: &str) -> Result<ResolvedService, String> {
        let current = self.snapshot.read().await;
        if current.loaded_at.elapsed() > self.max_stale {
            let sentence = format!(
                "service directory cache is stale (store generation {})",
                current.store_version
            );
            drop(current);
            // The refusal is evidence about the host this resolver could not
            // refresh FROM: a cache only goes stale because the authority
            // stopped answering, and `stado host link <authority>` is where
            // an operator looks for the reason. Detached, because every
            // resolution refuses while the cache is stale and a workload is
            // blocking on each one.
            let subject = self.source.read().await.subject_host(&self.local_target);
            host_silence::report_refusal_detached(
                subject,
                host_silence::READER_RESOLVER,
                host_silence::REASON_DIRECTORY_CACHE_STALE,
                sentence.clone(),
            );
            return Err(sentence);
        }
        service_resolution::resolve(&current.document, service, consumer)
    }

    fn gateway_url(&self, service: &str, consumer: &str) -> Option<String> {
        self.adapters
            .iter()
            .find(|adapter| adapter.service == service && adapter.consumer == consumer)
            .map(|adapter| format!("http://{}", adapter.bind))
    }

    /// Publish what this process holds right now.
    async fn publish_serving(&self) {
        let current = self.snapshot.read().await;
        publish(&PublishedState::serving(
            &self.local_target,
            current.directory_generation,
            &current.store_version,
            &current.loaded_at_iso,
        ));
    }
}

/// Drop the SSH control sockets this resolver left behind.
///
/// Multiplexing keeps a master alive for `ControlPersist` after the resolver
/// that opened it is gone. The next instance then attaches to a master whose
/// connection has already died, every proxied request fails with `Broken pipe`,
/// and the adapter answers nothing while looking perfectly healthy -- the same
/// failure the fleet had this morning from an orphaned port forward. Removing
/// the socket file costs nothing: live sessions keep their descriptor, and the
/// next connection opens a fresh master.
fn drop_stale_ssh_sockets() {
    let Ok(home) = std::env::var("HOME") else {
        return;
    };
    let directory = std::path::Path::new(&home).join(".stado");
    let Ok(entries) = std::fs::read_dir(&directory) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        if !name.starts_with("resolver-ssh-") {
            continue;
        }
        // Only sockets. `~/.stado/resolver-ssh-key` and its `.pub` share this
        // prefix, and deleting the resolver's own credential to clean up after
        // it is how a cleanup becomes the outage: the service then fails to
        // authenticate to the authority and exits before it can say why.
        let is_socket = entry
            .file_type()
            .map(|kind| {
                use std::os::unix::fs::FileTypeExt;
                kind.is_socket()
            })
            .unwrap_or(false);
        if !is_socket {
            continue;
        }
        match std::fs::remove_file(entry.path()) {
            Ok(()) => eprintln!("stado resolver dropped stale ssh control socket {name}"),
            Err(error) => eprintln!("stado resolver could not drop {name}: {error}"),
        }
    }
}

// ---------------------------------------------------------------------------
// What the resolver publishes about itself
// ---------------------------------------------------------------------------

/// Where `serve` publishes what it holds, under `~/.stado`.
///
/// Until 2026-08-19 the directory generation and the reason an upstream read
/// failed lived in this process's memory and in an 83 MiB stderr log, nowhere
/// else. So while this host's resolver sat in a launchd restart loop -- `last
/// exit code = 69: EX_UNAVAILABLE`, restarted on a five second
/// `ThrottleInterval` -- the two questions the operator had, which generation
/// it holds and why it cannot load another, had no answer anywhere in the
/// product. This file is the answer and [`status`] is its reader. It stays
/// readable with the resolver stopped, which is exactly when it gets read.
const STATE_FILE: &str = "resolver-state.json";

/// Operator override for [`STATE_FILE`]'s location, absolute.
const STATE_FILE_ENV: &str = "STADO_RESOLVER_STATE_FILE";

/// Serving traffic from a snapshot it holds.
const RESOLVER_SERVING: &str = "serving";
/// Reading its first snapshot; no port is bound yet.
const RESOLVER_STARTING: &str = "starting";
/// An upstream read failed and the next attempt is scheduled.
const RESOLVER_BACKING_OFF: &str = "backing_off";
/// Stopped for a reason no retry clears.
const RESOLVER_FAILED: &str = "failed";
/// No state file exists: no resolver has run since one was last removed.
/// Never written, only reported by [`status`].
const RESOLVER_UNPUBLISHED: &str = "unpublished";

/// First delay after a failed upstream read.
const BACKOFF_BASE: Duration = Duration::from_secs(1);

/// Ceiling on that delay.
///
/// Bounded in both directions, deliberately. The behaviour this replaces was
/// unbounded upward in restarts and downward in patience: `serve` exited 69,
/// launchd restarted it five seconds later, and the loop neither slowed down
/// nor said why. Retrying in place at a capped interval keeps one pid, one
/// log and one published reason, and a resolver that has been waiting an hour
/// still retries within the minute the authority comes back.
const BACKOFF_CAP: Duration = Duration::from_secs(60);

/// [`BACKOFF_BASE`] doubled per consecutive failure, capped at
/// [`BACKOFF_CAP`].
fn backoff_delay(attempt: u32) -> Duration {
    let doublings = attempt.saturating_sub(1).min(u32::BITS - 1);
    Duration::from_secs(
        BACKOFF_BASE
            .as_secs()
            .checked_shl(doublings)
            .unwrap_or(u64::MAX)
            .min(BACKOFF_CAP.as_secs()),
    )
}

/// `datetime.now(timezone.utc).isoformat()`, as every other writer in the
/// crate stamps it (`queue/leases.rs::now_iso`).
fn now_iso() -> String {
    chrono::Utc::now().to_rfc3339()
}

/// What one `resolver serve` process holds, and why it holds nothing more.
///
/// Read tolerantly (`serde(default)`): a newer resolver writing a field this
/// build does not model must not make [`status`] report a host with no
/// resolver at all, which is the strictness failure
/// `service_resolution::ServiceRoute` records at fleet scale.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
struct PublishedState {
    /// When this file was written.
    updated_at: String,
    /// Registry target whose `service_resolver` policy the process enforces.
    target: String,
    /// The process that wrote it.
    pid: u32,
    /// [`RESOLVER_SERVING`], [`RESOLVER_STARTING`], [`RESOLVER_BACKING_OFF`]
    /// or [`RESOLVER_FAILED`].
    state: String,
    /// Directory generation held, absent until one is.
    generation: Option<u64>,
    /// Registry store version that generation came from -- the value the
    /// adapter refusal quotes as `store generation 945077b5...`.
    store_version: Option<String>,
    /// When that snapshot was loaded.
    loaded_at: Option<String>,
    /// Why the process is not serving, in the upstream's own words.
    reason: Option<String>,
    /// Consecutive failed upstream reads.
    attempt: u32,
    /// When the next upstream read is due.
    next_attempt_at: Option<String>,
}

impl PublishedState {
    fn starting(target: &str) -> Self {
        Self {
            updated_at: now_iso(),
            target: target.to_string(),
            pid: std::process::id(),
            state: RESOLVER_STARTING.to_string(),
            ..Self::default()
        }
    }

    fn serving(target: &str, generation: u64, store_version: &str, loaded_at: &str) -> Self {
        Self {
            updated_at: now_iso(),
            target: target.to_string(),
            pid: std::process::id(),
            state: RESOLVER_SERVING.to_string(),
            generation: Some(generation),
            store_version: Some(store_version.to_string()),
            loaded_at: Some(loaded_at.to_string()),
            ..Self::default()
        }
    }

    fn backing_off(target: &str, attempt: u32, reason: &str, delay: Duration) -> Self {
        let ahead = chrono::Duration::from_std(delay).unwrap_or_else(|_| chrono::Duration::zero());
        Self {
            updated_at: now_iso(),
            target: target.to_string(),
            pid: std::process::id(),
            state: RESOLVER_BACKING_OFF.to_string(),
            reason: Some(reason.to_string()),
            attempt,
            next_attempt_at: Some((chrono::Utc::now() + ahead).to_rfc3339()),
            ..Self::default()
        }
    }

    fn failed(target: &str, reason: &str) -> Self {
        Self {
            updated_at: now_iso(),
            target: target.to_string(),
            pid: std::process::id(),
            state: RESOLVER_FAILED.to_string(),
            reason: Some(reason.to_string()),
            ..Self::default()
        }
    }
}

fn state_path() -> Option<std::path::PathBuf> {
    if let Some(explicit) = std::env::var_os(STATE_FILE_ENV) {
        let path = std::path::PathBuf::from(explicit);
        if !path.as_os_str().is_empty() {
            return Some(path);
        }
    }
    std::env::var_os("HOME")
        .map(std::path::PathBuf::from)
        .map(|home| home.join(".stado").join(STATE_FILE))
}

/// Publish the state, atomically, best effort.
///
/// Best effort on purpose: a resolver that cannot write its own diagnostic
/// file is still one that can serve traffic, and refusing to start over it
/// would make this file the outage. Written to a sibling and renamed rather
/// than in place, because [`status`] reads it while `serve` writes it and a
/// half-written document reads as a resolver that has never run.
fn publish(state: &PublishedState) {
    let Some(path) = state_path() else { return };
    let Some(parent) = path.parent() else { return };
    let body = match serde_json::to_vec_pretty(state) {
        Ok(body) => body,
        Err(error) => {
            eprintln!("stado resolver could not serialize its state: {error}");
            return;
        }
    };
    let temp = path.with_extension(format!("{}.tmp", std::process::id()));
    let written = std::fs::create_dir_all(parent)
        .and_then(|()| std::fs::write(&temp, &body))
        .and_then(|()| std::fs::rename(&temp, &path));
    if let Err(error) = written {
        let _ = std::fs::remove_file(&temp);
        eprintln!(
            "stado resolver could not publish its state to {}: {error}",
            path.display()
        );
    }
}

/// The state the last `serve` process published, when there is one.
fn published_state() -> Option<PublishedState> {
    let body = std::fs::read_to_string(state_path()?).ok()?;
    serde_json::from_str(&body).ok()
}

/// Why one startup read failed, and whether waiting can help.
enum StartupError {
    /// The same read succeeds later with nothing changed: the object API this
    /// host reads the registry through is not up yet, or the authority is
    /// asleep. Both happen on this fleet every day.
    Transient(String),
    /// Nothing a retry clears.
    Fatal(String),
}

/// Everything `serve` must read before it can bind one port.
struct Startup {
    source: SnapshotSource,
    document: Value,
    store_version: String,
    directory_generation: u64,
}

/// One attempt at everything the resolver must read before it can bind.
///
/// A document that fails `validate_registry` is [`StartupError::Transient`]
/// too, deliberately: the operator republishes the registry and this process
/// picks it up on the next attempt, where exiting would need a restart to
/// notice the fix. The reason is published either way, so a resolver waiting
/// on a malformed document is not a resolver waiting silently.
async fn load_startup(
    target: &str,
    local_store: &Arc<RegistryStore>,
) -> Result<Startup, StartupError> {
    let (bootstrap, _, _) = read_local_snapshot(local_store)
        .await
        .map_err(StartupError::Transient)?;
    let detected_target = current_target(&bootstrap).map_err(StartupError::Fatal)?;
    if detected_target != target {
        return Err(StartupError::Fatal(format!(
            "resolver target {target:?} does not match this host ({detected_target:?})"
        )));
    }
    let source =
        snapshot_source(Arc::clone(local_store), &bootstrap, target).map_err(StartupError::Fatal)?;
    let (document, store_version, directory_generation) = source
        .fetch(host_silence::READER_RESOLVER)
        .await
        .map_err(StartupError::Transient)?;
    Ok(Startup {
        source,
        document,
        store_version,
        directory_generation,
    })
}

/// [`load_startup`] behind a bounded backoff, publishing every refusal.
///
/// The two reads it wraps fail transiently and routinely, and neither is a
/// reason to exit: exiting 69 is what put this service in a restart loop
/// holding a dead ssh control socket, and only repeated `launchctl kickstart`
/// got it out. A registry that says this host is not the declared target
/// still exits immediately, because no amount of waiting fixes it.
async fn await_startup(
    target: &str,
    local_store: &Arc<RegistryStore>,
) -> Result<Startup, CmdError> {
    publish(&PublishedState::starting(target));
    let mut attempt = 0_u32;
    loop {
        match load_startup(target, local_store).await {
            Ok(startup) => return Ok(startup),
            Err(StartupError::Fatal(detail)) => {
                publish(&PublishedState::failed(target, &detail));
                return Err(CmdError::click(detail));
            }
            Err(StartupError::Transient(detail)) => {
                attempt = attempt.saturating_add(1);
                let delay = backoff_delay(attempt);
                eprintln!(
                    "stado resolver upstream read failed, attempt {attempt}, retrying in {}s: {detail}",
                    delay.as_secs()
                );
                publish(&PublishedState::backing_off(
                    target, attempt, &detail, delay,
                ));
                tokio::time::sleep(delay).await;
            }
        }
    }
}

pub async fn serve(target: &str) -> Result<(), CmdError> {
    drop_stale_ssh_sockets();
    let local_store = Arc::new(RegistryStore::open().await?);
    let Startup {
        source,
        document,
        store_version,
        directory_generation,
    } = await_startup(target, &local_store).await?;
    let config = match service_resolution::resolver_config(&document, target) {
        Ok(config) => config,
        Err(detail) => {
            publish(&PublishedState::failed(target, &detail));
            return Err(CmdError::click(detail));
        }
    };
    let state = Arc::new(ResolverState {
        local_store,
        source: RwLock::new(source),
        snapshot: RwLock::new(Snapshot {
            document,
            store_version,
            directory_generation,
            loaded_at: Instant::now(),
            loaded_at_iso: now_iso(),
        }),
        max_stale: Duration::from_secs(config.max_stale_seconds),
        local_target: target.to_string(),
        adapters: config.adapters.clone(),
        config: config.clone(),
    });

    // A resolver that must serve the very address it reads the registry through
    // cannot start in either order, and the two failures look unrelated: with
    // the object API up the bind fails with "address already in use", with it
    // down the read fails with "error sending request". On this workstation that
    // alternation ran 641 restarts while the desktop app quietly fell back to a
    // local vault and showed no subscriptions at all. Name the contradiction
    // once instead of oscillating between its two halves.
    let store_url = crate::config::wc_stado_storage_url();
    if let Ok(parsed) = url::Url::parse(store_url.trim()) {
        if let (Some(host), Some(port)) = (parsed.host_str(), parsed.port()) {
            let store_authority = format!("{host}:{port}");
            if let Some(adapter) = config
                .adapters
                .iter()
                .find(|adapter| adapter.bind.trim() == store_authority)
            {
                let detail = format!(
                    "this resolver is declared to serve {} for service {:?}, and the registry \
                     it must read first is configured at storage.stado.url = {}. One of the two \
                     has to move: either place the object API somewhere this resolver does not \
                     serve, or drop that adapter from the target's service_resolver policy. \
                     Retrying cannot resolve it.",
                    adapter.bind, adapter.service, store_url
                );
                publish(&PublishedState::failed(target, &detail));
                return Err(CmdError::click(detail));
            }
        }
    }

    let api = match bind_loopback(&config.api_bind).await {
        Ok(listener) => listener,
        Err(error) => {
            publish(&PublishedState::failed(target, &error.to_string()));
            return Err(error);
        }
    };
    let mut adapter_listeners = Vec::with_capacity(config.adapters.len());
    for adapter in &config.adapters {
        match bind_loopback(&adapter.bind).await {
            Ok(listener) => adapter_listeners.push((adapter.clone(), listener)),
            Err(error) => {
                publish(&PublishedState::failed(target, &error.to_string()));
                return Err(error);
            }
        }
    }

    // Published before the first port is accepted on, so `resolver status`
    // answers `serving` for exactly the window the sockets are open.
    state.publish_serving().await;

    eprintln!(
        "stado resolver target={} api={} adapters={} refresh={}s max-stale={}s",
        target,
        config.api_bind,
        config.adapters.len(),
        config.refresh_seconds,
        config.max_stale_seconds
    );

    let mut tasks = JoinSet::new();
    let refresh_state = Arc::clone(&state);
    tasks.spawn(async move { watch_registry(refresh_state, config.refresh_seconds).await });
    let api_state = Arc::clone(&state);
    tasks.spawn(async move { serve_api(api, api_state).await });
    for (adapter, listener) in adapter_listeners {
        let adapter_state = Arc::clone(&state);
        tasks.spawn(async move { serve_adapter(listener, adapter, adapter_state).await });
    }

    let exit = match tasks.join_next().await {
        Some(Ok(Ok(()))) => CmdError::click("resolver task exited unexpectedly"),
        Some(Ok(Err(error))) => CmdError::click(error),
        Some(Err(error)) => CmdError::click(format!("resolver task failed: {error}")),
        None => CmdError::click("resolver started no tasks"),
    };
    // The last thing this process says about itself. A task that died takes
    // the whole data plane with it, and leaving `serving` behind would make
    // `resolver status` vouch for a resolver that is gone.
    publish(&PublishedState::failed(target, &exit.to_string()));
    Err(exit)
}

async fn bind_loopback(value: &str) -> Result<TcpListener, CmdError> {
    let address: SocketAddr = value
        .parse()
        .map_err(|_| CmdError::click(format!("invalid resolver bind {value:?}")))?;
    if !address.ip().is_loopback() {
        return Err(CmdError::click(format!(
            "resolver bind {value:?} must be loopback"
        )));
    }
    TcpListener::bind(address).await.map_err(|error| {
        // A stable port held by something else is the failure this resolver
        // cannot recover from by waiting: a stale forward left behind by an
        // earlier instance accepts connections and answers none, so every
        // reader sees a live socket in front of nothing. Name the holder here
        // rather than retrying into it.
        CmdError::click(format!(
            "could not bind {value}: {error}. Something else already holds this \
             resolver port; find it with `lsof -nP -iTCP:{} -sTCP:LISTEN` and stop \
             it before starting the resolver.",
            address.port()
        ))
    })
}

/// Reload the snapshot on the declared interval, backing off when the
/// upstream will not answer.
///
/// The plain interval hammered a dead authority at `refresh_seconds`: on
/// 2026-08-19 that produced seven identical `registry authority exited with
/// exit status: 255: ssh: connect to host 100.120.25.24 port 22: Operation
/// timed out` lines, each costing a ten second ssh connect, and told nobody
/// anything the first had not. The reason is published once per attempt now,
/// and the wait between attempts grows to [`BACKOFF_CAP`]. Adapters keep
/// refusing with `service directory cache is stale` while this loop backs
/// off, which is the correct answer and no longer an unexplained one.
async fn watch_registry(state: Arc<ResolverState>, refresh_seconds: u64) -> Result<(), String> {
    let mut interval = tokio::time::interval(Duration::from_secs(refresh_seconds));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    interval.tick().await;
    let mut attempt = 0_u32;
    loop {
        interval.tick().await;
        match state.refresh().await {
            Ok(true) => {
                return Err(
                    "resolver configuration changed; restarting to rebind listeners".to_string(),
                )
            }
            Ok(false) => {
                if attempt != 0 {
                    eprintln!("stado resolver refresh recovered after {attempt} failed attempts");
                    attempt = 0;
                }
                state.publish_serving().await;
            }
            Err(error) => {
                attempt = attempt.saturating_add(1);
                let delay = backoff_delay(attempt);
                eprintln!(
                    "stado resolver refresh failed, attempt {attempt}, next in {}s: {error}",
                    delay.as_secs()
                );
                publish(&PublishedState::backing_off(
                    &state.local_target,
                    attempt,
                    &error,
                    delay,
                ));
                tokio::time::sleep(delay).await;
            }
        }
    }
}

async fn serve_adapter(
    listener: TcpListener,
    adapter: ResolverAdapter,
    state: Arc<ResolverState>,
) -> Result<(), String> {
    loop {
        let (client, _) = listener
            .accept()
            .await
            .map_err(|error| format!("{} accept failed: {error}", adapter.bind))?;
        let state = Arc::clone(&state);
        let adapter = adapter.clone();
        tokio::spawn(async move {
            if let Err(error) = proxy_connection(client, &adapter, &state).await {
                eprintln!(
                    "stado resolver adapter service={} consumer={} rejected connection: {}",
                    adapter.service, adapter.consumer, error
                );
            }
        });
    }
}

async fn copy_until_idle<R, W>(
    reader: &mut R,
    writer: &mut W,
    idle: Duration,
) -> Result<bool, std::io::Error>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let mut buffer = [0_u8; 16 * 1024];
    loop {
        let read = match tokio::time::timeout(idle, reader.read(&mut buffer)).await {
            Ok(result) => result?,
            Err(_) => {
                writer.shutdown().await?;
                return Ok(true);
            }
        };
        if read == 0 {
            writer.shutdown().await?;
            return Ok(false);
        }
        match tokio::time::timeout(idle, writer.write_all(&buffer[..read])).await {
            Ok(result) => result?,
            Err(_) => {
                writer.shutdown().await?;
                return Ok(true);
            }
        }
    }
}

/// Terse refusal for clients that speak HTTP, sent before the prompt close.
const UPSTREAM_REFUSAL: &[u8] = b"HTTP/1.1 502 Bad Gateway\r\nContent-Type: text/plain\r\nContent-Length: 21\r\nConnection: close\r\n\r\nupstream unavailable\n";

/// Brief window to sniff the client's first bytes on a refusal where the
/// establishment phase never read any. The connection is already dead, so the
/// only cost of a miss is closing without the 502 body.
const REFUSAL_SNIFF: Duration = Duration::from_millis(250);

/// The stream speaks HTTP when its first bytes open with a request method.
fn http_request_head(head: &[u8]) -> bool {
    const METHODS: [&[u8]; 9] = [
        b"GET ",
        b"POST ",
        b"PUT ",
        b"DELETE ",
        b"HEAD ",
        b"OPTIONS ",
        b"PATCH ",
        b"CONNECT ",
        b"TRACE ",
    ];
    METHODS.iter().any(|method| head.starts_with(method))
}

/// Surface an unreachable upstream instead of letting the client hang: one
/// line naming the service, endpoint, and cause, then an HTTP 502 when the
/// connection's first bytes are an HTTP request, and a prompt close either
/// way. A refused connection is a handled outcome, so callers return `Ok(())`
/// rather than doubling the line through `serve_adapter`.
async fn refuse_connection<R, W>(
    reader: &mut R,
    writer: &mut W,
    adapter: &ResolverAdapter,
    endpoint: &str,
    cause: &str,
    head: Option<&[u8]>,
) where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    eprintln!(
        "stado resolver service={} consumer={} endpoint={} refused connection: {}",
        adapter.service, adapter.consumer, endpoint, cause
    );
    let mut sniff = [0_u8; 512];
    let head = match head {
        Some(head) => Some(head),
        None => match tokio::time::timeout(REFUSAL_SNIFF, reader.read(&mut sniff)).await {
            Ok(Ok(read)) if read > 0 => Some(&sniff[..read]),
            _ => None,
        },
    };
    if head.map_or(false, http_request_head) {
        let _ = writer.write_all(UPSTREAM_REFUSAL).await;
    }
    let _ = writer.shutdown().await;
}

async fn proxy_connection(
    client: TcpStream,
    adapter: &ResolverAdapter,
    state: &ResolverState,
) -> Result<(), String> {
    let resolved = state.resolve(&adapter.service, &adapter.consumer).await?;
    eprintln!(
        "stado resolver service={} consumer={} generation={} destination={}",
        adapter.service, adapter.consumer, resolved.generation, resolved.active_host
    );
    let endpoint = url::Url::parse(&resolved.endpoint.url)
        .map_err(|error| format!("invalid resolved endpoint: {error}"))?;
    let host = endpoint
        .host_str()
        .ok_or_else(|| "resolved endpoint has no host".to_string())?;
    let port = endpoint
        .port_or_known_default()
        .ok_or_else(|| "resolved endpoint has no port".to_string())?;
    let idle = Duration::from_secs(adapter.idle_seconds);
    // The directory is supposed to name where a service listens. When it names
    // this adapter's own bind instead, the proxy dials itself: the connection is
    // accepted, forwarded to the same socket, accepted again, and the reader
    // waits on a chain that never reaches a server. It looks exactly like a
    // healthy port with a hung backend, so say it instead of recursing.
    if format!("{host}:{port}") == adapter.bind {
        return Err(format!(
            "service {} resolves to {}, which is this adapter's own bind: the service \
             directory must carry the address the service listens on, not the stable \
             port published for its clients",
            adapter.service, adapter.bind
        ));
    }
    if resolved.active_host == state.local_target {
        let (mut client_read, mut client_write) = client.into_split();
        let upstream = match TcpStream::connect((host, port)).await {
            Ok(upstream) => upstream,
            Err(error) => {
                refuse_connection(
                    &mut client_read,
                    &mut client_write,
                    adapter,
                    &format!("{host}:{port}"),
                    &format!("local upstream connect failed: {error}"),
                    None,
                )
                .await;
                return Ok(());
            }
        };
        let (mut upstream_read, mut upstream_write) = upstream.into_split();
        let upload = copy_until_idle(&mut client_read, &mut upstream_write, idle);
        let download = copy_until_idle(&mut upstream_read, &mut client_write, idle);
        tokio::try_join!(upload, download)
            .map_err(|error| format!("local proxy failed: {error}"))?;
        return Ok(());
    }

    let ssh = resolved.ssh.ok_or_else(|| {
        format!(
            "active host {:?} has no registry SSH transport",
            resolved.active_host
        )
    })?;
    let destination = format!("{host}:{port}");
    let mut child = ssh_command()
        .args(["-W", &destination, &ssh])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .kill_on_drop(true)
        .spawn()
        .map_err(|error| format!("could not start registry SSH transport: {error}"))?;
    let mut ssh_stdin = child
        .stdin
        .take()
        .ok_or_else(|| "SSH transport has no stdin".to_string())?;
    let mut ssh_stdout = child
        .stdout
        .take()
        .ok_or_else(|| "SSH transport has no stdout".to_string())?;
    let (mut client_read, mut client_write) = client.into_split();
    let connect = Duration::from_secs(adapter.connect_seconds);
    // Establishment is the one window `idle_seconds` cannot bound: nothing has
    // flowed yet, so a dead backend would otherwise hold the client for the
    // whole idle window. Forward whatever the client sends so the upstream has
    // a request to answer, then wait -- bounded by the connect budget -- for
    // the first upstream byte, or for the SSH child's early exit: `ssh -W`
    // opens its channel at startup, so a dead backend fails the channel open
    // and the child is gone within moments.
    let mut upstream_head = [0_u8; 16 * 1024];
    let mut client_head: Option<Vec<u8>> = None;
    let establishment = async {
        let mut buffer = [0_u8; 16 * 1024];
        loop {
            tokio::select! {
                status = child.wait() => {
                    let status = status
                        .map_err(|error| format!("SSH transport wait failed: {error}"))?;
                    return Err(format!(
                        "SSH transport exited before the upstream answered: {status}"
                    ));
                }
                read = ssh_stdout.read(&mut upstream_head) => {
                    return match read {
                        Ok(0) => Err(
                            "SSH transport closed before the upstream answered".to_string()
                        ),
                        Ok(read) => Ok(read),
                        Err(error) => Err(format!("SSH transport read failed: {error}")),
                    };
                }
                read = client_read.read(&mut buffer) => {
                    match read {
                        Ok(0) => {
                            return Err(
                                "client closed before the upstream answered".to_string()
                            )
                        }
                        Ok(read) => {
                            if client_head.is_none() {
                                client_head = Some(buffer[..read].to_vec());
                            }
                            ssh_stdin.write_all(&buffer[..read]).await.map_err(|error| {
                                format!("SSH transport write failed: {error}")
                            })?;
                        }
                        Err(error) => return Err(format!("client read failed: {error}")),
                    }
                }
            }
        }
    };
    let established = match tokio::time::timeout(connect, establishment).await {
        Ok(Ok(read)) => read,
        Ok(Err(cause)) => {
            let _ = child.kill().await;
            let _ = child.wait().await;
            refuse_connection(
                &mut client_read,
                &mut client_write,
                adapter,
                &destination,
                &cause,
                client_head.as_deref(),
            )
            .await;
            return Ok(());
        }
        Err(_) => {
            let _ = child.kill().await;
            let _ = child.wait().await;
            refuse_connection(
                &mut client_read,
                &mut client_write,
                adapter,
                &destination,
                &format!(
                    "no upstream bytes within the {}s connect budget",
                    adapter.connect_seconds
                ),
                client_head.as_deref(),
            )
            .await;
            return Ok(());
        }
    };
    client_write
        .write_all(&upstream_head[..established])
        .await
        .map_err(|error| format!("client write failed: {error}"))?;
    let upload = copy_until_idle(&mut client_read, &mut ssh_stdin, idle);
    let download = copy_until_idle(&mut ssh_stdout, &mut client_write, idle);
    let (upload_idle, download_idle) =
        tokio::try_join!(upload, download).map_err(|error| format!("SSH proxy failed: {error}"))?;
    if upload_idle || download_idle {
        let _ = child.kill().await;
        let _ = child.wait().await;
        return Ok(());
    }
    let status = child
        .wait()
        .await
        .map_err(|error| format!("SSH transport wait failed: {error}"))?;
    if !status.success() {
        return Err(format!("SSH transport exited with {status}"));
    }
    Ok(())
}

async fn serve_api(listener: TcpListener, state: Arc<ResolverState>) -> Result<(), String> {
    loop {
        let (mut stream, _) = listener
            .accept()
            .await
            .map_err(|error| format!("resolution API accept failed: {error}"))?;
        let state = Arc::clone(&state);
        tokio::spawn(async move {
            let response = match read_request(&mut stream).await {
                Ok(request) => handle_api_request(request, &state).await,
                Err(error) => api_response(400, json!({"error": error})),
            };
            if let Err(error) = stream.write_all(&response).await {
                eprintln!("stado resolver API write failed: {error}");
            }
        });
    }
}

struct ApiRequest {
    method: String,
    path: String,
    headers: BTreeMap<String, String>,
}

async fn read_request(stream: &mut TcpStream) -> Result<ApiRequest, String> {
    let mut bytes = Vec::new();
    let mut chunk = [0u8; 2048];
    loop {
        let read = stream
            .read(&mut chunk)
            .await
            .map_err(|error| format!("request read failed: {error}"))?;
        if read == 0 {
            return Err("request ended before HTTP head".to_string());
        }
        bytes.extend_from_slice(&chunk[..read]);
        if bytes.len() > REQUEST_HEAD_LIMIT {
            return Err("request head is too large".to_string());
        }
        if bytes.windows(4).any(|window| window == b"\r\n\r\n") {
            break;
        }
    }
    let head = String::from_utf8(bytes).map_err(|_| "request head is not UTF-8".to_string())?;
    let mut lines = head.split("\r\n");
    let request_line = lines
        .next()
        .ok_or_else(|| "request line is missing".to_string())?;
    let mut request_parts = request_line.split_whitespace();
    let method = request_parts
        .next()
        .ok_or_else(|| "request method is missing".to_string())?
        .to_string();
    let path = request_parts
        .next()
        .ok_or_else(|| "request path is missing".to_string())?
        .to_string();
    let mut headers = BTreeMap::new();
    for line in lines.take_while(|line| !line.is_empty()) {
        let (name, value) = line
            .split_once(':')
            .ok_or_else(|| "invalid request header".to_string())?;
        headers.insert(name.trim().to_ascii_lowercase(), value.trim().to_string());
    }
    Ok(ApiRequest {
        method,
        path,
        headers,
    })
}

async fn handle_api_request(request: ApiRequest, state: &ResolverState) -> Vec<u8> {
    if request.method != "GET" {
        return api_response(405, json!({"error": "method_not_allowed"}));
    }
    if request.path == "/health" {
        let current = state.snapshot.read().await;
        if current.loaded_at.elapsed() > state.max_stale {
            return api_response(503, json!({"status": "stale"}));
        }
        return api_response(
            200,
            json!({
                "status": "ok",
                "service": "stado-resolver",
                "generation": current.directory_generation,
            }),
        );
    }
    let Some(service) = request.path.strip_prefix("/v1/resolve/service/") else {
        return api_response(404, json!({"error": "not_found"}));
    };
    if service.is_empty() || service.contains('/') || service.contains('?') {
        return api_response(400, json!({"error": "invalid_service"}));
    }
    let Some(consumer) = request.headers.get("x-stado-consumer") else {
        return api_response(401, json!({"error": "consumer_required"}));
    };
    match state.resolve(service, consumer).await {
        Ok(resolved) => api_response(
            200,
            json!({
                "service": format!("stado://service/{}", resolved.name),
                "generation": resolved.generation,
                "gateway_url": state.gateway_url(service, consumer),
                "capabilities": resolved.capabilities,
            }),
        ),
        Err(error) => api_response(503, json!({"error": error})),
    }
}

fn api_response(status: u16, body: Value) -> Vec<u8> {
    let reason = match status {
        200 => "OK",
        400 => "Bad Request",
        401 => "Unauthorized",
        404 => "Not Found",
        405 => "Method Not Allowed",
        _ => "Service Unavailable",
    };
    let body = serde_json::to_vec(&body).unwrap_or_else(|_| b"{}".to_vec());
    let head = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let mut response = head.into_bytes();
    response.extend_from_slice(&body);
    response
}

// ---------------------------------------------------------------------------
// `resolver status` — readiness, answerable with the resolver stopped
// ---------------------------------------------------------------------------

/// How long a bind probe waits for a loopback connect. A listener on loopback
/// answers in microseconds or is not there; this is slack for a loaded
/// machine, not a network budget.
const BIND_PROBE: Duration = Duration::from_millis(250);

/// Budget for the whole authority probe. [`ssh_command`] already carries
/// `ConnectTimeout=10`; this bounds everything after the connect, so a
/// diagnostic can never hang on the thing it is diagnosing.
const AUTHORITY_PROBE: Duration = Duration::from_secs(20);

/// Whether something is accepting connections at a declared bind.
async fn bind_listening(bind: &str) -> bool {
    let Ok(address) = bind.trim().parse::<SocketAddr>() else {
        return false;
    };
    matches!(
        tokio::time::timeout(BIND_PROBE, TcpStream::connect(address)).await,
        Ok(Ok(_))
    )
}

/// Seconds since an ISO 8601 stamp, `None` when it does not parse.
fn age_seconds(stamp: &str) -> Option<i64> {
    let then = chrono::DateTime::parse_from_rfc3339(stamp).ok()?;
    Some((chrono::Utc::now() - then.with_timezone(&chrono::Utc)).num_seconds())
}

/// Where the authority's document came from, and whether it answered.
struct AuthorityAnswer {
    /// `"local"` when this host is the authority, `"ssh"` otherwise.
    source: &'static str,
    reachable: bool,
    /// The generation the authority publishes, when it answered.
    generation: Option<u64>,
    /// Why it did not, verbatim.
    detail: Option<String>,
}

/// Ask the registry authority for the generation it publishes.
///
/// When this host IS the authority there is nothing to ask: the document in
/// hand came from the authority read, and whether that read reached the store
/// or fell back to the last-known-good copy is already known — `notice` is
/// `Some` exactly when it fell back.
async fn probe_authority(
    registry: &targets::Registry,
    directory: &service_resolution::ServiceDirectory,
    target: &str,
    notice: Option<&str>,
) -> AuthorityAnswer {
    if directory.authority.target == target {
        return AuthorityAnswer {
            source: "local",
            reachable: notice.is_none(),
            generation: Some(directory.generation),
            detail: notice.map(str::to_string),
        };
    }
    let Some(ssh) = registry
        .lookup(&directory.authority.target)
        .and_then(|authority| authority.ssh.clone())
    else {
        return AuthorityAnswer {
            source: "ssh",
            reachable: false,
            generation: None,
            detail: Some(format!(
                "registry target {:?} has no SSH transport",
                directory.authority.target
            )),
        };
    };
    let source = SnapshotSource::Authority {
        target: directory.authority.target.clone(),
        ssh,
        command: directory.authority.command.clone(),
    };
    match tokio::time::timeout(AUTHORITY_PROBE, source.fetch(host_silence::READER_CLI)).await {
        Ok(Ok((_, _, generation))) => AuthorityAnswer {
            source: "ssh",
            reachable: true,
            generation: Some(generation),
            detail: None,
        },
        Ok(Err(detail)) => AuthorityAnswer {
            source: "ssh",
            reachable: false,
            generation: None,
            detail: Some(detail),
        },
        Err(_) => AuthorityAnswer {
            source: "ssh",
            reachable: false,
            generation: None,
            detail: Some(format!(
                "no answer from the registry authority within {}s",
                AUTHORITY_PROBE.as_secs()
            )),
        },
    }
}

/// `stado resolver status` — the four facts an operator needs about a local
/// resolver, and a non-zero exit when any of them is wrong.
///
/// The registry comes through [`targets::fetch_registry_or_last_good`]: a
/// command whose whole purpose is diagnosing a sick control plane must not die
/// with the authority it is diagnosing, and every host command did exactly
/// that on 2026-08-19. A cached answer is still an answer, and its age is a
/// blocker in the report rather than a footnote.
async fn status(target: Option<&str>, json_output: bool) -> Result<(), CmdError> {
    let (registry, notice) = targets::fetch_registry_or_last_good()
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    if let Some(notice) = notice.as_deref() {
        targets::report_registry_notice(notice);
    }
    let registry_staleness = registry.staleness_seconds;
    let document = registry.to_document();
    let published = published_state();
    let target = match target {
        Some(target) => target.to_string(),
        None => match published
            .as_ref()
            .map(|state| state.target.as_str())
            .filter(|target| !target.is_empty())
        {
            Some(target) => target.to_string(),
            None => current_target(&document).map_err(CmdError::click)?,
        },
    };
    let config =
        service_resolution::resolver_config(&document, &target).map_err(CmdError::click)?;
    let directory = service_resolution::directory(&document)
        .map_err(CmdError::click)?
        .ok_or_else(|| CmdError::click("registry.service_directory is required"))?;

    let api_listening = bind_listening(&config.api_bind).await;
    let mut probed: Vec<(&ResolverAdapter, bool)> = Vec::with_capacity(config.adapters.len());
    for adapter in &config.adapters {
        let listening = bind_listening(&adapter.bind).await;
        probed.push((adapter, listening));
    }
    let authority = probe_authority(&registry, &directory, &target, notice.as_deref()).await;

    let resolver_state = published
        .as_ref()
        .map(|state| state.state.as_str())
        .filter(|state| !state.is_empty())
        .unwrap_or(RESOLVER_UNPUBLISHED)
        .to_string();
    let held = published.as_ref().and_then(|state| state.generation);
    let held_age = published
        .as_ref()
        .and_then(|state| state.loaded_at.as_deref())
        .and_then(age_seconds);
    let behind = match (held, authority.generation) {
        (Some(held), Some(publishes)) if held < publishes => Some((held, publishes)),
        _ => None,
    };
    let past_window = held_age.is_some_and(|age| age > config.max_stale_seconds as i64);
    // Holding no generation at all counts: a resolver that has never loaded
    // the directory is not fresh, it is absent, and reporting `stale: false`
    // for it would vouch for a data plane that cannot resolve one name.
    let stale = held.is_none() || behind.is_some() || past_window;

    let mut blockers: Vec<String> = Vec::new();
    match &published {
        None => blockers.push(format!(
            "no resolver has published state at {}: nothing has served here since that file was \
             last removed",
            state_path()
                .map(|path| path.display().to_string())
                .unwrap_or_else(|| STATE_FILE.to_string())
        )),
        Some(state) if resolver_state != RESOLVER_SERVING => {
            let mut sentence = format!("the resolver reports state {resolver_state}");
            if let Some(reason) = state.reason.as_deref() {
                sentence.push_str(": ");
                sentence.push_str(reason);
            }
            if let Some(next) = state.next_attempt_at.as_deref() {
                sentence.push_str(&format!(
                    " (failed attempt {}, next read due {next})",
                    state.attempt
                ));
            }
            blockers.push(sentence);
        }
        Some(_) => {}
    }
    if !api_listening {
        blockers.push(format!(
            "nothing is listening on the resolution API at {}",
            config.api_bind
        ));
    }
    for (adapter, listening) in &probed {
        if !listening {
            blockers.push(format!(
                "nothing is listening on the {} adapter for consumer {} at {}",
                adapter.service, adapter.consumer, adapter.bind
            ));
        }
    }
    if !authority.reachable {
        let mut sentence = format!(
            "the registry authority {} is unreachable",
            directory.authority.target
        );
        if let Some(detail) = authority.detail.as_deref() {
            sentence.push_str(": ");
            sentence.push_str(detail);
        }
        blockers.push(sentence);
    }
    if let Some((held, publishes)) = behind {
        blockers.push(format!(
            "the resolver holds service directory generation {held} and the authority publishes \
             {publishes}"
        ));
    }
    if past_window {
        blockers.push(format!(
            "the snapshot the resolver holds is {}s old, past the {}s max-stale window this \
             target declares",
            held_age.unwrap_or_default(),
            config.max_stale_seconds
        ));
    }
    if let Some(seconds) = registry_staleness {
        blockers.push(format!(
            "this answer read a registry copy {seconds}s old rather than the authority"
        ));
    }

    // `down` is reserved for a resolver that is answering nothing at all.
    // Everything else that is wrong is `degraded`, because an adapter short of
    // its upstream still serves the services whose upstream is up.
    let verdict = if blockers.is_empty() {
        "ready"
    } else if !api_listening && resolver_state != RESOLVER_SERVING {
        "down"
    } else {
        "degraded"
    };

    let report = json!({
        "target": target,
        "state": resolver_state,
        "pid": published.as_ref().map(|state| state.pid).filter(|pid| *pid != 0),
        "updated_at": published.as_ref().map(|state| state.updated_at.clone()),
        "api": {"bind": config.api_bind, "listening": api_listening},
        "adapters": probed
            .iter()
            .map(|(adapter, listening)| json!({
                "service": adapter.service,
                "consumer": adapter.consumer,
                "bind": adapter.bind,
                "listening": listening,
            }))
            .collect::<Vec<Value>>(),
        "authority": {
            "target": directory.authority.target,
            "source": authority.source,
            "reachable": authority.reachable,
            "generation": authority.generation,
            "detail": authority.detail,
        },
        "generation": held,
        "generation_age_seconds": held_age,
        "max_stale_seconds": config.max_stale_seconds,
        "stale": stale,
        "registry_staleness_seconds": registry_staleness,
        "reason": published.as_ref().and_then(|state| state.reason.clone()),
        "attempt": published.as_ref().map_or(0, |state| state.attempt),
        "next_attempt_at": published.as_ref().and_then(|state| state.next_attempt_at.clone()),
        "verdict": verdict,
        "blockers": blockers,
    });

    if json_output {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!(
            "resolver {target} state={resolver_state} verdict={verdict} generation={} stale={}",
            held.map_or_else(|| "-".to_string(), |generation| generation.to_string()),
            if stale { "yes" } else { "no" }
        );
        println!(
            "api {} {}",
            config.api_bind,
            if api_listening {
                "listening"
            } else {
                "not-listening"
            }
        );
        for (adapter, listening) in &probed {
            println!(
                "adapter {} consumer={} {} {}",
                adapter.service,
                adapter.consumer,
                adapter.bind,
                if *listening {
                    "listening"
                } else {
                    "not-listening"
                }
            );
        }
        println!(
            "authority {} source={} {} generation={}",
            directory.authority.target,
            authority.source,
            if authority.reachable {
                "reachable"
            } else {
                "unreachable"
            },
            authority
                .generation
                .map_or_else(|| "-".to_string(), |generation| generation.to_string())
        );
        for blocker in &blockers {
            println!("blocker {blocker}");
        }
    }

    if verdict == "ready" {
        return Ok(());
    }
    // The report is the answer; a second `Error:` line restating it would be
    // the third copy of one fact on one screen.
    Err(CmdError::silent(super::CLICK_ERROR_CODE))
}
