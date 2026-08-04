//! `stado resolver` — logical service discovery and local data plane.

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::process::Stdio;
use std::sync::Arc;
use std::time::{Duration, Instant};

use clap::Subcommand;
use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::process::Command;
use tokio::sync::RwLock;
use tokio::task::JoinSet;

use crate::service_resolution::{self, ResolvedService, ResolverAdapter};
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
}

pub async fn dispatch(command: ResolverCommands) -> Result<(), CmdError> {
    match command {
        ResolverCommands::Resolve {
            service,
            consumer,
            json,
        } => resolve_once(&service, &consumer, json).await,
        ResolverCommands::Serve { target } => serve(&target).await,
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

async fn fetch_snapshot(store: &RegistryStore) -> Result<(Value, String, u64), String> {
    let blob = store
        .read_versioned()
        .await
        .map_err(|error| format!("registry read failed: {error}"))?
        .ok_or_else(|| format!("no registry document at {}", store.location()))?;
    let document: Value = serde_json::from_str(&blob.content)
        .map_err(|error| format!("invalid registry JSON: {error}"))?;
    targets::validate_registry(&document).map_err(|error| error.to_string())?;
    let directory = service_resolution::directory(&document)?
        .ok_or_else(|| "registry.service_directory is required".to_string())?;
    Ok((document, blob.version, directory.generation))
}

async fn resolve_once(service: &str, consumer: &str, json_output: bool) -> Result<(), CmdError> {
    let service = logical_name(service)?;
    let store = RegistryStore::open().await?;
    let (document, _, _) = fetch_snapshot(&store).await.map_err(CmdError::click)?;
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
    store: Arc<RegistryStore>,
    snapshot: RwLock<Snapshot>,
    max_stale: Duration,
    local_target: String,
    adapters: Vec<ResolverAdapter>,
}

impl ResolverState {
    async fn refresh(&self) -> Result<(), String> {
        let (document, store_version, generation) = fetch_snapshot(&self.store).await?;
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
        Ok(())
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

pub async fn serve(target: &str) -> Result<(), CmdError> {
    let store = Arc::new(RegistryStore::open().await?);
    let (document, store_version, directory_generation) =
        fetch_snapshot(&store).await.map_err(CmdError::click)?;
    let parsed_registry =
        targets::load_registry_from_str(&serde_json::to_string(&document).map_err(CmdError::from)?)
            .map_err(|error| CmdError::click(error.to_string()))?;
    let hostname = crate::providers::vast::system_hostname();
    let self_target = parsed_registry
        .lookup_self(&hostname)
        .map_err(|error| CmdError::click(error.to_string()))?
        .ok_or_else(|| {
            CmdError::click(format!(
                "resolver host {hostname:?} has no registry target identity"
            ))
        })?;
    if self_target.name != target {
        return Err(CmdError::click(format!(
            "resolver target {target:?} does not match this host ({:?})",
            self_target.name
        )));
    }
    let config = service_resolution::resolver_config(&document, target).map_err(CmdError::click)?;
    let state = Arc::new(ResolverState {
        store,
        snapshot: RwLock::new(Snapshot {
            document,
            store_version,
            directory_generation,
            loaded_at: Instant::now(),
        }),
        max_stale: Duration::from_secs(config.max_stale_seconds),
        local_target: target.to_string(),
        adapters: config.adapters.clone(),
    });

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
    tasks.spawn(async move {
        watch_registry(refresh_state, config.refresh_seconds).await;
        Ok::<(), String>(())
    });
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
    TcpListener::bind(address)
        .await
        .map_err(|error| CmdError::click(format!("could not bind {value}: {error}")))
}

async fn watch_registry(state: Arc<ResolverState>, refresh_seconds: u64) {
    let mut interval = tokio::time::interval(Duration::from_secs(refresh_seconds));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    interval.tick().await;
    loop {
        interval.tick().await;
        if let Err(error) = state.refresh().await {
            eprintln!("stado resolver refresh failed: {error}");
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

    if resolved.active_host == state.local_target {
        let mut upstream = TcpStream::connect((host, port))
            .await
            .map_err(|error| format!("local upstream connect failed: {error}"))?;
        let mut client = client;
        tokio::io::copy_bidirectional(&mut client, &mut upstream)
            .await
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
    let mut command = Command::new("ssh");
    command.args([
        "-T",
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
    if let Ok(key_file) = std::env::var("STADO_RESOLVER_SSH_KEY_FILE") {
        if !key_file.trim().is_empty() {
            command
                .args(["-o", "IdentitiesOnly=yes", "-i"])
                .arg(key_file);
        }
    }
    let mut child = command
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
    let upload = tokio::io::copy(&mut client_read, &mut ssh_stdin);
    let download = tokio::io::copy(&mut ssh_stdout, &mut client_write);
    tokio::try_join!(upload, download).map_err(|error| format!("SSH proxy failed: {error}"))?;
    drop(ssh_stdin);
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
