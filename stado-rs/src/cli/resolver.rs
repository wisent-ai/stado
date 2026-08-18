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

async fn read_local_snapshot(store: &RegistryStore) -> Result<(Value, String, u64), String> {
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

#[derive(Clone)]
enum SnapshotSource {
    Local(Arc<RegistryStore>),
    Authority { ssh: String, command: String },
}

impl SnapshotSource {
    async fn fetch(&self) -> Result<(Value, String, u64), String> {
        match self {
            Self::Local(store) => read_local_snapshot(store).await,
            Self::Authority { ssh, command } => {
                let remote_command =
                    format!("{} resolver snapshot", crate::deploy::shlex_quote(command));
                let output = ssh_command()
                    .arg(ssh)
                    .arg(remote_command)
                    .stderr(Stdio::piped())
                    .output()
                    .await
                    .map_err(|error| format!("registry authority SSH failed: {error}"))?;
                if !output.status.success() {
                    let detail = String::from_utf8_lossy(&output.stderr);
                    let detail = detail.trim();
                    return Err(if detail.is_empty() {
                        format!("registry authority exited with {}", output.status)
                    } else {
                        format!(
                            "registry authority exited with {}: {}",
                            output.status,
                            detail.chars().take(4096).collect::<String>()
                        )
                    });
                }
                if output.stdout.len() > SNAPSHOT_LIMIT {
                    return Err("registry authority snapshot exceeds 1 MiB".to_string());
                }
                let payload: SnapshotPayload = serde_json::from_slice(&output.stdout)
                    .map_err(|error| format!("invalid registry authority response: {error}"))?;
                validate_snapshot(payload)
            }
        }
    }
}

fn parsed_registry(document: &Value) -> Result<targets::Registry, String> {
    targets::load_registry_from_str(
        &serde_json::to_string(document).map_err(|error| error.to_string())?,
    )
    .map_err(|error| error.to_string())
}

fn current_target(document: &Value) -> Result<String, String> {
    let hostname = crate::providers::vast::system_hostname();
    parsed_registry(document)?
        .lookup_self(&hostname)
        .map_err(|error| error.to_string())?
        .map(|target| target.name.clone())
        .ok_or_else(|| format!("resolver host {hostname:?} has no registry target identity"))
}

fn snapshot_source(
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
    let (document, _, _) = source.fetch().await.map_err(CmdError::click)?;
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
        let (document, store_version, generation) = source.fetch().await?;
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
        drop(current);
        *self.source.write().await = next_source;
        Ok(false)
    }

    async fn resolve(&self, service: &str, consumer: &str) -> Result<ResolvedService, String> {
        let current = self.snapshot.read().await;
        if current.loaded_at.elapsed() > self.max_stale {
            return Err(format!(
                "service directory cache is stale (store generation {})",
                current.store_version
            ));
        }
        service_resolution::resolve(&current.document, service, consumer)
    }

    fn gateway_url(&self, service: &str, consumer: &str) -> Option<String> {
        self.adapters
            .iter()
            .find(|adapter| adapter.service == service && adapter.consumer == consumer)
            .map(|adapter| format!("http://{}", adapter.bind))
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

pub async fn serve(target: &str) -> Result<(), CmdError> {
    drop_stale_ssh_sockets();
    let local_store = Arc::new(RegistryStore::open().await?);
    let (bootstrap, _, _) = read_local_snapshot(&local_store)
        .await
        .map_err(CmdError::click)?;
    let detected_target = current_target(&bootstrap).map_err(CmdError::click)?;
    if detected_target != target {
        return Err(CmdError::click(format!(
            "resolver target {target:?} does not match this host ({detected_target:?})"
        )));
    }
    let source =
        snapshot_source(Arc::clone(&local_store), &bootstrap, target).map_err(CmdError::click)?;
    let (document, store_version, directory_generation) =
        source.fetch().await.map_err(CmdError::click)?;
    let config = service_resolution::resolver_config(&document, target).map_err(CmdError::click)?;
    let state = Arc::new(ResolverState {
        local_store,
        source: RwLock::new(source),
        snapshot: RwLock::new(Snapshot {
            document,
            store_version,
            directory_generation,
            loaded_at: Instant::now(),
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
                return Err(CmdError::click(format!(
                    "this resolver is declared to serve {} for service {:?}, and the registry \
                     it must read first is configured at storage.stado.url = {}. One of the two \
                     has to move: either place the object API somewhere this resolver does not \
                     serve, or drop that adapter from the target's service_resolver policy. \
                     Retrying cannot resolve it.",
                    adapter.bind, adapter.service, store_url
                )));
            }
        }
    }

    let api = bind_loopback(&config.api_bind).await?;
    let mut adapter_listeners = Vec::with_capacity(config.adapters.len());
    for adapter in &config.adapters {
        adapter_listeners.push((adapter.clone(), bind_loopback(&adapter.bind).await?));
    }

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

    match tasks.join_next().await {
        Some(Ok(Ok(()))) => Err(CmdError::click("resolver task exited unexpectedly")),
        Some(Ok(Err(error))) => Err(CmdError::click(error)),
        Some(Err(error)) => Err(CmdError::click(format!("resolver task failed: {error}"))),
        None => Err(CmdError::click("resolver started no tasks")),
    }
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

async fn watch_registry(state: Arc<ResolverState>, refresh_seconds: u64) -> Result<(), String> {
    let mut interval = tokio::time::interval(Duration::from_secs(refresh_seconds));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    interval.tick().await;
    loop {
        interval.tick().await;
        match state.refresh().await {
            Ok(true) => {
                return Err(
                    "resolver configuration changed; restarting to rebind listeners".to_string(),
                )
            }
            Ok(false) => {}
            Err(error) => eprintln!("stado resolver refresh failed: {error}"),
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
        let upstream = TcpStream::connect((host, port))
            .await
            .map_err(|error| format!("local upstream connect failed: {error}"))?;
        let (mut client_read, mut client_write) = client.into_split();
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
