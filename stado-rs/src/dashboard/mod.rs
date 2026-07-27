//! HTTP operator dashboard for the wisent-compute queue and disk cleanup.
//! Port of `stado/dashboard.py` (ThreadingHTTPServer read-only dashboard).
//!
//! GET /                  - HTML overview (auto-refresh)
//! GET /api/state.json    - queue dashboard data as JSON
//! GET /api/cleanup.json  - sanitized current cleanup state
//! GET /api/artifacts.json / GET /api/artifact.json?ref=
//! GET /api/registry.json - policy-safe canonical registry projection
//! POST /api/cleanup/run  - one parameterless registry-controlled cleanup pass
//! POST /api/registry/policy - whitelisted generation-checked policy mutation
//! GET /healthz           - liveness (before auth, after the Host guard)
//! GET /livez             - Cloud Run liveness alias
//!
//! Summary and rendering helpers live in `dashboard/summary.rs` +
//! `dashboard/web_view.rs` (Python `dashboard_summary/`). This module holds
//! only the HTTP plumbing and the refresh loop.
//!
//! DEVIATIONS from Python (deliberate):
//! - Hand-rolled minimal HTTP/1.1 on `tokio::net::TcpListener`, one task per
//!   accepted connection (the ThreadingHTTPServer equivalent); the port
//!   spec forbids adding a web-framework dependency. Python's implicit
//!   `Server:`/`Date:` response headers and its error-page HTML bodies are
//!   not reproduced (status codes and JSON bodies match).
//! - Registry policy writes use the storage backend's generation/version CAS
//!   rather than a process-local lock, so concurrent dashboard revisions
//!   cannot overwrite each other.

pub mod policy;
pub mod summary;
pub mod web_view;

use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

use crate::artifacts::registry::{ArtifactRegistry, RegistryError as ArtifactRegistryError};
use crate::artifacts_models::ArtifactRef;
use crate::config;
use crate::models::isoformat_utc;
use crate::providers::local::disk_cleanup::{
    read_cleanup_state, run_cleanup_once, sanitize_cleanup_report,
};
use crate::queue::submit::json_dumps_sorted_compact;
use crate::queue::{python_json_dumps, JobStorage, StorageError};

/// Dashboard serve/refresh failure.
#[derive(Debug, thiserror::Error)]
pub enum DashboardError {
    /// Storage failures from the summarizer / artifact listing.
    #[error(transparent)]
    Storage(#[from] StorageError),
    /// Listener/socket failures.
    #[error(transparent)]
    Io(#[from] std::io::Error),
    /// Artifact registry failures (corrupt manifests, alias resolution).
    #[error(transparent)]
    Artifacts(#[from] ArtifactRegistryError),
    /// Cleanup-state read failures and port validation.
    #[error("{0}")]
    Other(String),
}

/// Background refresh state (Python `_CACHE_STATE`): populated by the
/// refresh task and read by the request handlers. The slow path
/// (`_summarize`) downloads every job blob, so it never runs inline with a
/// request; we serve the last cached snapshot and refresh in the
/// background.
#[derive(Clone)]
pub struct Dashboard {
    store: JobStorage,
    state: Arc<RwLock<Value>>,
    refresh_seconds: i64,
}

/// The Python `_CACHE_STATE` initial shape.
fn initial_state(bucket: &str) -> Value {
    json!({
        "ready": false,
        "now": Value::Null,
        "bucket": bucket,
        "counts": {"queue": 0, "running": 0, "completed": 0, "failed": 0},
        "by_model_state": {},
        "live_agents": [],
        "stale_agents": [],
        "recent_failed": [],
        "completed_recent": [],
        "artifacts": [],
        "throughput": {
            "avg_wall_seconds_per_completed_job": Value::Null,
            "samples": 0,
            "live_total_free_slots": 0,
            "projected_remaining_seconds": Value::Null,
        },
        "last_refresh_seconds": Value::Null,
    })
}

impl Dashboard {
    /// Bind the dashboard to a storage facade. The auto-refresh interval
    /// comes from `config::dashboard_refresh_seconds()`.
    pub fn new(store: JobStorage) -> Self {
        Self {
            state: Arc::new(RwLock::new(initial_state(store.bucket_name()))),
            store,
            refresh_seconds: config::dashboard_refresh_seconds(),
        }
    }

    /// Override the auto-refresh interval (tests; Python passes
    /// DASHBOARD_REFRESH_SECONDS to the refresher thread).
    pub fn with_refresh_seconds(mut self, seconds: i64) -> Self {
        self.refresh_seconds = seconds;
        self
    }

    /// Current cached snapshot (Python `dict(_CACHE_STATE)`).
    pub fn snapshot(&self) -> Value {
        self.state.read().expect("dashboard state lock").clone()
    }

    /// One refresh pass (Python `_refresh_loop` body): fast counts first so
    /// /api/state.json can return SOMETHING quickly, then the artifact list,
    /// then the full per-job summary.
    pub async fn refresh_once(&self) -> Result<(), DashboardError> {
        let t0 = Instant::now();
        let counts = summary::fast_counts(&self.store).await?;
        {
            let mut state = self.state.write().expect("dashboard state lock");
            let state = state.as_object_mut().expect("state object");
            if let Some(target) = state.get_mut("counts").and_then(Value::as_object_mut) {
                target.extend(counts);
            }
            state.insert("now".to_string(), json!(isoformat_utc(chrono::Utc::now())));
            state.insert("ready".to_string(), json!(true));
        }
        let registry = ArtifactRegistry::with_store(self.store.clone());
        let mut artifacts = Vec::new();
        for manifest in registry.list("", "", "", &[]).await? {
            let primary = manifest
                .locations
                .iter()
                .find(|location| location.role == "primary")
                .map(|location| location.uri.clone())
                .unwrap_or_default();
            artifacts.push(json!({
                "ref": manifest.ref_.to_string(),
                "title": manifest.title,
                "aliases": registry.aliases_for(&manifest.ref_).await?,
                "verification": manifest.verification.result,
                "run_id": manifest.producer.run_id,
                "primary_uri": primary,
                "summary": manifest.summary,
                "created_at": manifest.created_at,
            }));
        }
        {
            let mut state = self.state.write().expect("dashboard state lock");
            state
                .as_object_mut()
                .expect("state object")
                .insert("artifacts".to_string(), Value::Array(artifacts));
        }
        let full = summary::summarize(&self.store).await?;
        let mut state = self.state.write().expect("dashboard state lock");
        let state = state.as_object_mut().expect("state object");
        // Python `_CACHE_STATE.update(full)`.
        if let Value::Object(full) = full {
            state.extend(full);
        }
        state.insert(
            "last_refresh_seconds".to_string(),
            json!(t0.elapsed().as_secs_f64()),
        );
        state.insert("ready".to_string(), json!(true));
        Ok(())
    }

    /// Python `_refresh_loop`: refresh every `interval_seconds`. Refresh
    /// failures crash the loop — the task dies and the HTTP handlers keep
    /// serving the last good cached snapshot until the operator restarts
    /// the dashboard. (Python previously logged-and-continued, silently
    /// producing a stale dashboard indefinitely.)
    pub async fn run_refresh_loop(self, interval_seconds: u64) {
        loop {
            if let Err(exc) = self.refresh_once().await {
                eprintln!("[dashboard] refresh loop died: {exc}");
                return;
            }
            tokio::time::sleep(Duration::from_secs(interval_seconds)).await;
        }
    }

    /// Python `serve`: start the refresh daemon, bind, accept forever.
    pub async fn serve_with(&self, host: &str, port: u16) -> Result<(), DashboardError> {
        // The refresh loop's future trips a `&str` lifetime-generalization
        // issue in the artifact-listing chain (Send "not general enough"),
        // so — like Python's daemon refresher thread — it runs on its own
        // OS thread with a current-thread runtime instead of tokio::spawn.
        let refresher = self.clone();
        let interval = self.refresh_seconds.max(1) as u64;
        std::thread::Builder::new()
            .name("dashboard-refresh".to_string())
            .spawn(move || {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .expect("dashboard refresh runtime");
                runtime.block_on(refresher.run_refresh_loop(interval));
            })?;
        let listener = TcpListener::bind((host, port)).await?;
        eprintln!("[dashboard] listening on http://{host}:{port}");
        self.serve_on(listener).await
    }

    /// Accept loop on an already-bound listener (tests bind 127.0.0.1:0).
    /// One task per connection — the ThreadingHTTPServer equivalent.
    pub async fn serve_on(&self, listener: TcpListener) -> Result<(), DashboardError> {
        loop {
            let (stream, _) = listener.accept().await?;
            let dashboard = self.clone();
            tokio::spawn(async move {
                if let Err(exc) = dashboard.handle_connection(stream).await {
                    eprintln!("[dashboard] connection error: {exc}");
                }
            });
        }
    }

    async fn handle_connection(&self, mut stream: TcpStream) -> std::io::Result<()> {
        let Some(request) = read_request(&mut stream).await? else {
            return Ok(());
        };
        let response = self.route(&request).await;
        eprintln!(
            "[dashboard] \"{} {} HTTP/1.1\" {} -",
            request.method, request.path, response.status
        );
        stream.write_all(&response.bytes).await?;
        stream.shutdown().await
    }

    async fn route(&self, request: &Request) -> Response {
        match request.method.as_str() {
            "" => empty_response(400, "Bad Request"),
            "GET" => self.do_get(request).await,
            "POST" => self.do_post(request).await,
            // Python BaseHTTPRequestHandler: 501 Unsupported method.
            _ => empty_response(501, "Not Implemented"),
        }
    }

    async fn do_get(&self, request: &Request) -> Response {
        if !trusted_request_host(request.header("host"), request.header("x-forwarded-proto")) {
            return cleanup_failure(403);
        }
        let path_no_query = request.path.split('?').next().unwrap_or("");
        if path_no_query == "/healthz" || path_no_query == "/livez" {
            return send_json(200, &json!({"ok": true}));
        }
        if !authorized(request, "view").await {
            return send_json(401, &json!({"error": "unauthorized"}));
        }
        match self.get_routes(request).await {
            Ok(response) => response,
            // Python: a failing /api/cleanup.json answers the safe cleanup
            // envelope; every other route answers 500 "dashboard error".
            Err(_) if request.path == "/api/cleanup.json" => cleanup_failure(500),
            Err(_) => Response::text(500, "Internal Server Error", "dashboard error"),
        }
    }

    async fn get_routes(&self, request: &Request) -> Result<Response, DashboardError> {
        let state = self.snapshot();
        let (path, query) = match request.path.split_once('?') {
            Some((path, query)) => (path, query),
            None => (request.path.as_str(), ""),
        };
        if path == "/api/artifacts.json" {
            let artifacts = state.get("artifacts").cloned().unwrap_or_else(|| json!([]));
            let body = python_json_dumps(&artifacts)
                .map_err(|exc| DashboardError::Other(exc.to_string()))?;
            return Ok(Response::json(200, &body));
        }
        if path == "/api/artifact.json" {
            let ref_value = parse_qs(query)
                .into_iter()
                .filter(|(key, value)| key == "ref" && !value.is_empty())
                .map(|(_, value)| value)
                .next()
                .unwrap_or_default();
            if ref_value.is_empty() {
                return Ok(empty_response(400, "Bad Request"));
            }
            let registry = ArtifactRegistry::with_store(self.store.clone());
            let reference = ArtifactRef::parse(&ref_value).map_err(ArtifactRegistryError::from)?;
            let manifest = registry.resolve_manifest(&reference).await?;
            let mut value = manifest.to_dict();
            let map = value.as_object_mut().expect("manifest object");
            map.insert("requested_ref".to_string(), json!(ref_value));
            map.insert(
                "aliases".to_string(),
                json!(registry.aliases_for(&manifest.ref_).await?),
            );
            return Ok(Response::json(
                200,
                &python_json_dumps(&value).map_err(|exc| DashboardError::Other(exc.to_string()))?,
            ));
        }
        if request.path == "/api/state.json" {
            return Ok(send_json(200, &state));
        }
        if request.path == "/api/registry.json" {
            return Ok(
                match policy::policy_view(self.store.backend().as_ref()).await {
                    Ok(value) => send_json(http_status("200"), &value),
                    Err(error) => send_json(error.status(), &json!({"error": error.to_string()})),
                },
            );
        }
        if request.path == "/api/cleanup.json" {
            let report =
                read_cleanup_state().map_err(|exc| DashboardError::Other(exc.to_string()))?;
            let payload = web_view::cleanup_envelope(&report);
            let status = if payload["service"] == "busy" {
                409
            } else {
                200
            };
            return Ok(send_json(status, &payload));
        }
        if request.path == "/" || request.path == "/index.html" {
            let report =
                read_cleanup_state().map_err(|exc| DashboardError::Other(exc.to_string()))?;
            let cleanup = web_view::cleanup_envelope(&report);
            let body = web_view::render_html(&state, &cleanup, self.refresh_seconds);
            return Ok(Response::html(200, &body));
        }
        Ok(empty_response(404, "Not Found"))
    }

    async fn do_post(&self, request: &Request) -> Response {
        if !trusted_request_host(request.header("host"), request.header("x-forwarded-proto")) {
            return cleanup_failure(403);
        }
        if !authorized(request, "operate").await {
            return send_json(401, &json!({"error": "unauthorized"}));
        }
        let path_no_query = request.path.split('?').next().unwrap_or("");
        if path_no_query == "/api/registry/policy" {
            return self.post_registry_policy(request).await;
        }
        if request.path != "/api/cleanup/run" {
            let path_no_query = request.path.split('?').next().unwrap_or("");
            return if path_no_query == "/api/cleanup/run" {
                cleanup_failure(400)
            } else {
                empty_response(404, "Not Found")
            };
        }
        match self.post_cleanup_run(request).await {
            Ok(response) => response,
            Err(_) => cleanup_failure(500),
        }
    }

    async fn post_registry_policy(&self, request: &Request) -> Response {
        if request.path != "/api/registry/policy"
            || request.header("x-stado-action") != Some("registry-policy")
        {
            return send_json(
                http_status("403"),
                &json!({"ok": false, "error": "forbidden"}),
            );
        }
        let content_type = request
            .header("content-type")
            .unwrap_or("")
            .split(';')
            .next()
            .unwrap_or("")
            .trim();
        let content_length = request
            .header("content-length")
            .and_then(|value| value.parse::<usize>().ok());
        if content_type != "application/json"
            || request.header("transfer-encoding").is_some()
            || content_length != Some(request.body.len())
        {
            return send_json(
                http_status("400"),
                &json!({"ok": false, "error": "invalid JSON request framing"}),
            );
        }
        let payload: Value = match serde_json::from_slice(&request.body) {
            Ok(payload) => payload,
            Err(error) => {
                return send_json(
                    http_status("400"),
                    &json!({"ok": false, "error": format!("invalid JSON: {error}")}),
                )
            }
        };
        match policy::update_policy(self.store.backend().as_ref(), &payload).await {
            Ok(value) => send_json(http_status("200"), &value),
            Err(error) => send_json(
                error.status(),
                &json!({"ok": false, "error": error.to_string()}),
            ),
        }
    }
    async fn post_cleanup_run(&self, request: &Request) -> Result<Response, DashboardError> {
        if request.header("x-stado-action") != Some("cleanup") {
            return Ok(cleanup_failure(403));
        }
        let content_length: i64 = match request
            .header("content-length")
            .unwrap_or("0")
            .trim()
            .parse()
        {
            Ok(n) => n,
            Err(_) => return Ok(cleanup_failure(400)),
        };
        if content_length != 0 || request.header("transfer-encoding").is_some() {
            return Ok(cleanup_failure(400));
        }
        // run_cleanup_once holds a `&mut dyn FnMut` logger across awaits,
        // which makes its future non-Send — so the pass runs on a dedicated
        // thread with its own current-thread runtime instead of inline in
        // the connection task. One thread per operator click, matching
        // Python's ThreadingHTTPServer handler thread.
        let (tx, rx) = tokio::sync::oneshot::channel();
        std::thread::Builder::new()
            .name("dashboard-cleanup-run".to_string())
            .spawn(move || {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .expect("cleanup runtime");
                let mut log_fn = |msg: &str| eprintln!("[dashboard] {msg}");
                let report = runtime.block_on(run_cleanup_once(0, true, &mut log_fn));
                let _ = tx.send(report);
            })?;
        let report = rx
            .await
            .map_err(|_| DashboardError::Other("cleanup pass thread died".to_string()))?;
        let report = sanitize_cleanup_report(&report);
        let payload = web_view::cleanup_envelope(&report);
        let status = if payload["service"] == "busy" {
            409
        } else {
            200
        };
        Ok(send_json(status, &payload))
    }
}

/// Python `serve(host=None, port=None)`: run the dashboard HTTP server.
/// Blocks until killed. Defaults from `config::dashboard_bind()` /
/// `config::dashboard_port()`; storage from `config::bucket()`.
pub async fn serve(host: Option<&str>, port: Option<i64>) -> Result<(), DashboardError> {
    let host = host
        .map(str::to_string)
        .unwrap_or_else(|| config::dashboard_bind().to_string());
    let port = port.unwrap_or_else(config::dashboard_port);
    let port = u16::try_from(port)
        .map_err(|_| DashboardError::Other(format!("dashboard port out of range: {port}")))?;
    let store = JobStorage::new().await?;
    Dashboard::new(store).serve_with(&host, port).await
}

// ---------------------------------------------------------------------------
// Minimal hand-rolled HTTP/1.1 (no framework dependency, per the port spec)
// ---------------------------------------------------------------------------

/// Request head cap (Python's http.server parses a similar 64 KiB budget).
const MAX_HEAD_BYTES: usize = 65536;

struct Request {
    method: String,
    /// Raw request target including the query string (Python `self.path`).
    path: String,
    /// (lowercased name, trimmed value), first occurrence wins.
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl Request {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    }
}

fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

/// Read one request with a bounded head and body. Body framing is deliberately
/// limited to Content-Length; mutating routes reject Transfer-Encoding.
async fn read_request(stream: &mut TcpStream) -> std::io::Result<Option<Request>> {
    let mut buf: Vec<u8> = Vec::new();
    let mut tmp = [0u8; 8192];
    let head_end = loop {
        let n = stream.read(&mut tmp).await?;
        if n == 0 {
            if buf.is_empty() {
                return Ok(None);
            }
            return Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "incomplete HTTP request head",
            ));
        }
        buf.extend_from_slice(&tmp[..n]);
        if let Some(pos) = find_subslice(&buf, b"\r\n\r\n") {
            break pos;
        }
        if buf.len() > MAX_HEAD_BYTES {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "HTTP request head too large",
            ));
        }
    };
    let head = String::from_utf8_lossy(&buf[..head_end]).into_owned();
    let mut lines = head.split("\r\n");
    let request_line = lines.next().unwrap_or("");
    let mut parts = request_line.split_whitespace();
    let (method, path) = match (parts.next(), parts.next(), parts.next()) {
        (Some(method), Some(path), Some(version)) if version.starts_with("HTTP/") => {
            (method.to_string(), path.to_string())
        }
        _ => (String::new(), String::new()),
    };
    let mut headers = Vec::new();
    for line in lines {
        if let Some((name, value)) = line.split_once(':') {
            let name = name.trim().to_ascii_lowercase();
            if !headers.iter().any(|(key, _)| *key == name) {
                headers.push((name, value.trim().to_string()));
            }
        }
    }
    let body_start = head_end + b"\r\n\r\n".len();
    let content_length = headers
        .iter()
        .find(|(name, _)| name == "content-length")
        .and_then(|(_, value)| value.parse::<usize>().ok())
        .unwrap_or_default();
    if content_length > MAX_HEAD_BYTES {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "HTTP request body too large",
        ));
    }
    let available = buf.len().saturating_sub(body_start).min(content_length);
    let mut body = Vec::with_capacity(content_length);
    body.extend_from_slice(&buf[body_start..body_start + available]);
    if body.len() < content_length {
        let missing = content_length - body.len();
        let mut remainder = vec![u8::default(); missing];
        stream.read_exact(&mut remainder).await?;
        body.extend_from_slice(&remainder);
    }
    Ok(Some(Request {
        method,
        path,
        headers,
        body,
    }))
}

struct Response {
    status: u16,
    bytes: Vec<u8>,
}

impl Response {
    fn new(status: u16, reason: &str, content_type: &str, body: &[u8]) -> Self {
        let head = format!(
            "HTTP/1.1 {status} {reason}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        let mut bytes = head.into_bytes();
        bytes.extend_from_slice(body);
        Self { status, bytes }
    }

    fn json(status: u16, body: &str) -> Self {
        let reason = match status {
            200 => "OK",
            401 => "Unauthorized",
            409 => "Conflict",
            _ => "OK",
        };
        Self::new(status, reason, "application/json", body.as_bytes())
    }

    fn html(status: u16, body: &str) -> Self {
        Self::new(status, "OK", "text/html; charset=utf-8", body.as_bytes())
    }

    fn text(status: u16, reason: &str, body: &str) -> Self {
        Self::new(status, reason, "text/plain; charset=utf-8", body.as_bytes())
    }
}

fn http_status(value: &str) -> u16 {
    value.parse().expect("static HTTP status is valid")
}

fn empty_response(status: u16, reason: &str) -> Response {
    Response::new(status, reason, "text/plain; charset=utf-8", b"")
}

/// Python `_Handler._send_json`:
/// `json.dumps(payload, default=str, sort_keys=True, separators=(",", ":"))`.
fn send_json(status: u16, payload: &Value) -> Response {
    Response::json(status, &json_dumps_sorted_compact(payload))
}

/// Python `_cleanup_failure`.
fn cleanup_failure(status: u16) -> Response {
    let report = sanitize_cleanup_report(&json!({"outcome": "invalid_or_unavailable_policy"}));
    send_json(
        status,
        &json!({"ok": false, "service": "error", "report": report}),
    )
}

/// Python `parse_qs(urlsplit(self.path).query)`: `&`-separated `key=value`
/// pairs, percent-decoded with `+` as space.
fn parse_qs(query: &str) -> Vec<(String, String)> {
    query
        .split('&')
        .filter(|pair| !pair.is_empty())
        .map(|pair| {
            let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
            (url_decode(key), url_decode(value))
        })
        .collect()
}

fn url_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b'%' => {
                let hex = |b: u8| (b as char).to_digit(16);
                match (
                    bytes.get(i + 1).and_then(|b| hex(*b)),
                    bytes.get(i + 2).and_then(|b| hex(*b)),
                ) {
                    (Some(hi), Some(lo)) => {
                        out.push((hi * 16 + lo) as u8);
                        i += 3;
                    }
                    _ => {
                        out.push(b'%');
                        i += 1;
                    }
                }
            }
            byte => {
                out.push(byte);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

// ---------------------------------------------------------------------------
// Host-header DNS-rebinding guard + Supabase RLS auth
// ---------------------------------------------------------------------------

/// Python `_trusted_request_host`: reject rebinding locally; allow
/// authenticated HTTPS reverse proxies (deployment set + X-Forwarded-Proto
/// https). IPs pass directly; DNS names only in the reverse-proxy case.
fn trusted_request_host(value: Option<&str>, forwarded_proto: Option<&str>) -> bool {
    let Some(value) = value else { return false };
    if value.is_empty() {
        return false;
    }
    // Python's ValueError escape hatch (bad bracket, bad port, non-IP DNS
    // name): allowed only behind the authenticated reverse proxy.
    let dns_branch = !config::stado_deployment_id().is_empty() && forwarded_proto == Some("https");

    // authority = [userinfo@]host[:port]; path/query/fragment split off.
    let (authority, has_suffix) = match value.find(['/', '?', '#']) {
        Some(index) => (&value[..index], true),
        None => (value, false),
    };
    let (userinfo, host_port) = match authority.rsplit_once('@') {
        Some((userinfo, host_port)) => (Some(userinfo), host_port),
        None => (None, authority),
    };
    let (host, port) = if let Some(rest) = host_port.strip_prefix('[') {
        match rest.split_once(']') {
            Some((host, after)) => {
                let port = if after.is_empty() {
                    None
                } else if let Some(port) = after.strip_prefix(':') {
                    Some(port)
                } else {
                    // "[::1]junk" — Python raises ValueError on .hostname.
                    return dns_branch;
                };
                (host, port)
            }
            // Unterminated bracket — urlsplit raises ValueError.
            None => return dns_branch,
        }
    } else {
        match host_port.rsplit_once(':') {
            Some((host, port)) => (host, Some(port)),
            None => (host_port, None),
        }
    };
    // Python `_ = parsed.port`: an unparseable/out-of-range port raises
    // ValueError -> the DNS branch (even for IP hosts).
    if let Some(port) = port {
        if port.parse::<u16>().is_err() {
            return dns_branch;
        }
    }
    // Python: `if not host or parsed.username or parsed.password or
    // parsed.path or parsed.query or parsed.fragment: return False`.
    let (username, password) = match userinfo {
        Some(userinfo) => match userinfo.split_once(':') {
            Some((name, password)) => (name, Some(password)),
            None => (userinfo, None),
        },
        None => ("", None),
    };
    if host.is_empty()
        || !username.is_empty()
        || password.is_some_and(|password| !password.is_empty())
        || has_suffix
    {
        return false;
    }
    if host.parse::<std::net::IpAddr>().is_ok() {
        return true;
    }
    dns_branch
}

/// Python `_authorized`: validate the Wisent session and deployment grant
/// through Supabase RLS. No deployment configured = open (local dashboard);
/// otherwise fail CLOSED on any error.
async fn authorized(request: &Request, permission: &str) -> bool {
    let deployment_id = config::stado_deployment_id();
    if deployment_id.is_empty() {
        return true;
    }
    let supabase_url = std::env::var("SUPABASE_URL").unwrap_or_default();
    let supabase_url = supabase_url.trim_end_matches('/');
    let anon_key = std::env::var("SUPABASE_ANON_KEY")
        .unwrap_or_default()
        .trim()
        .to_string();
    let authorization = request.header("authorization").unwrap_or("").trim();
    if supabase_url.is_empty() || anon_key.is_empty() || !authorization.starts_with("Bearer ") {
        return false;
    }
    let body = json!({
        "target_deployment_id": deployment_id,
        "requested_permission": permission,
    });
    let result = reqwest::Client::new()
        .post(format!("{supabase_url}/rest/v1/rpc/stado_can_access"))
        .header("apikey", anon_key)
        .header("Authorization", authorization)
        .header("Content-Type", "application/json")
        .body(body.to_string())
        .timeout(Duration::from_secs(5))
        .send()
        .await;
    let Ok(response) = result else { return false };
    if response.status() != reqwest::StatusCode::OK {
        return false;
    }
    response
        .json::<Value>()
        .await
        .map(|value| value == Value::Bool(true))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use crate::queue::LocalBackend;

    async fn lock() -> tokio::sync::MutexGuard<'static, ()> {
        crate::testutil::GLOBAL_STATE_LOCK.lock().await
    }

    fn store() -> (tempfile::TempDir, JobStorage) {
        let dir = tempfile::tempdir().expect("tempdir");
        let backend = LocalBackend::new(dir.path().to_str().expect("utf8 path")).expect("backend");
        let store = JobStorage::with_backend_and_bucket(Arc::new(backend), "local", "test-bucket");
        (dir, store)
    }

    /// Bind a dashboard on a loopback ephemeral port and spawn the accept
    /// loop; returns the base URL plus the dashboard handle (refresh is NOT
    /// auto-started — tests call `refresh_once` explicitly).
    async fn spawn_server(dashboard: &Dashboard) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("addr");
        let server = dashboard.clone();
        tokio::spawn(async move {
            let _ = server.serve_on(listener).await;
        });
        format!("http://{addr}")
    }

    /// Save/restore env vars mutated by the auth tests.
    struct EnvGuard(Vec<(&'static str, Option<String>)>);

    impl EnvGuard {
        fn set(vars: &[(&'static str, &str)]) -> Self {
            let saved = vars
                .iter()
                .map(|(key, _)| (*key, std::env::var(key).ok()))
                .collect();
            for (key, value) in vars {
                std::env::set_var(key, value);
            }
            Self(saved)
        }

        fn unset(vars: &[&'static str]) -> Self {
            let saved = vars
                .iter()
                .map(|key| (*key, std::env::var(key).ok()))
                .collect();
            for key in vars {
                std::env::remove_var(key);
            }
            Self(saved)
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            for (key, value) in &self.0 {
                match value {
                    Some(value) => std::env::set_var(key, value),
                    None => std::env::remove_var(key),
                }
            }
        }
    }

    #[tokio::test]
    async fn healthz_and_state_json_shape() {
        let _guard = lock().await;
        let _env = EnvGuard::unset(&["STADO_DEPLOYMENT_ID"]);
        let (_dir, store) = store();
        // One fabricated queued job so fast counts are non-trivial.
        let job = crate::models::Job::new("job00001", "echo hi");
        store.write_job("queue", &job).await.unwrap();

        let dashboard = Dashboard::new(store).with_refresh_seconds(10);
        dashboard.refresh_once().await.expect("refresh");
        let base = spawn_server(&dashboard).await;
        let client = reqwest::Client::new();

        let health = client.get(format!("{base}/healthz")).send().await.unwrap();
        assert_eq!(health.status(), 200);
        assert_eq!(health.json::<Value>().await.unwrap(), json!({"ok": true}));

        let state = client
            .get(format!("{base}/api/state.json"))
            .send()
            .await
            .unwrap();
        assert_eq!(state.status(), 200);
        let state = state.json::<Value>().await.unwrap();
        assert_eq!(state["ready"], true);
        assert_eq!(state["bucket"], "test-bucket");
        assert_eq!(state["counts"]["queue"], 1);
        assert_eq!(state["counts"]["running"], 0);
        // "echo hi" has no --model flag -> the "(unknown)" bucket.
        assert_eq!(state["by_model_state"]["(unknown)"]["queue"], 1);
        assert!(state["throughput"]["samples"].is_number());
        assert!(state["last_refresh_seconds"].is_number());

        // Unknown path -> 404.
        let missing = client.get(format!("{base}/nope")).send().await.unwrap();
        assert_eq!(missing.status(), 404);

        // GET / serves the operator HTML.
        let html = client.get(format!("{base}/")).send().await.unwrap();
        assert_eq!(html.status(), 200);
        let body = html.text().await.unwrap();
        assert!(body.contains("Stado Control Center"));
    }

    #[tokio::test]
    async fn artifacts_endpoints_over_fabricated_manifests() {
        let _guard = lock().await;
        let _env = EnvGuard::unset(&["STADO_DEPLOYMENT_ID"]);
        let (_dir, store) = store();
        // Upload the manifest blob directly (publish() would run validation).
        let reference = ArtifactRef::parse("activations/wisent/acts@v1").unwrap();
        let manifest = crate::artifacts_models::ArtifactManifest::new(reference, "acts");
        store
            .upload_text(
                "artifacts/manifests/activations/wisent/acts/v1.json",
                &serde_json::to_string(&manifest.to_dict()).unwrap(),
            )
            .await
            .unwrap();

        let dashboard = Dashboard::new(store).with_refresh_seconds(10);
        dashboard.refresh_once().await.expect("refresh");
        let base = spawn_server(&dashboard).await;
        let client = reqwest::Client::new();

        let list = client
            .get(format!("{base}/api/artifacts.json"))
            .send()
            .await
            .unwrap();
        assert_eq!(list.status(), 200);
        let list = list.json::<Value>().await.unwrap();
        assert_eq!(list[0]["ref"], "activations/wisent/acts@v1");

        let detail = client
            .get(format!(
                "{base}/api/artifact.json?ref=activations/wisent/acts@v1"
            ))
            .send()
            .await
            .unwrap();
        assert_eq!(detail.status(), 200);
        let detail = detail.json::<Value>().await.unwrap();
        assert_eq!(detail["requested_ref"], "activations/wisent/acts@v1");

        // Missing ref -> 400.
        let missing = client
            .get(format!("{base}/api/artifact.json"))
            .send()
            .await
            .unwrap();
        assert_eq!(missing.status(), 400);
    }

    #[tokio::test]
    async fn host_header_guard_rejects_dns_names_without_deployment() {
        let _guard = lock().await;
        let _env = EnvGuard::unset(&["STADO_DEPLOYMENT_ID"]);
        let (_dir, store) = store();
        let dashboard = Dashboard::new(store).with_refresh_seconds(10);
        let base = spawn_server(&dashboard).await;
        let client = reqwest::Client::new();

        // Evil DNS Host header -> 403 with the safe cleanup envelope.
        let response = client
            .get(format!("{base}/api/state.json"))
            .header("Host", "evil.example.com")
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 403);
        let body = response.json::<Value>().await.unwrap();
        assert_eq!(body["ok"], false);
        assert_eq!(body["service"], "error");
        assert_eq!(body["report"]["outcome"], "invalid_or_unavailable_policy");

        // /healthz is guarded too (Python checks the Host guard first).
        let response = client
            .get(format!("{base}/healthz"))
            .header("Host", "evil.example.com")
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 403);
    }

    #[tokio::test]
    async fn auth_gate_with_loopback_supabase_mock() {
        let _guard = lock().await;
        let supabase = crate::testutil::mock_http(vec![
            crate::testutil::http_response(200, "OK", "true"),
            crate::testutil::http_response(200, "OK", "false"),
        ])
        .await;
        let _env = EnvGuard::set(&[
            ("STADO_DEPLOYMENT_ID", "dep-1"),
            ("SUPABASE_URL", &supabase.base_url),
            ("SUPABASE_ANON_KEY", "anon-key"),
        ]);
        let (_dir, store) = store();
        let dashboard = Dashboard::new(store).with_refresh_seconds(10);
        let base = spawn_server(&dashboard).await;
        let client = reqwest::Client::new();

        // No bearer token -> 401.
        let response = client
            .get(format!("{base}/api/state.json"))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 401);
        assert_eq!(
            response.json::<Value>().await.unwrap(),
            json!({"error": "unauthorized"})
        );

        // Bearer accepted by the RPC (returns true) -> 200.
        let response = client
            .get(format!("{base}/api/state.json"))
            .header("Authorization", "Bearer good-token")
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200);

        // RPC returns false -> 401.
        let response = client
            .get(format!("{base}/api/state.json"))
            .header("Authorization", "Bearer bad-token")
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 401);

        // The RPC call carried the deployment id, permission and apikey.
        let requests = supabase.requests.lock().unwrap();
        assert_eq!(requests.len(), 2);
        assert!(requests[0].starts_with("POST /rest/v1/rpc/stado_can_access HTTP/1.1"));
        assert!(requests[0].contains("apikey: anon-key"));
        assert!(requests[0].contains("authorization: Bearer good-token"));
        assert!(requests[0]
            .contains("{\"target_deployment_id\":\"dep-1\",\"requested_permission\":\"view\"}"));
    }

    #[tokio::test]
    async fn post_routes_unknown_and_cleanup_run_guards() {
        let _guard = lock().await;
        let _env = EnvGuard::unset(&["STADO_DEPLOYMENT_ID"]);
        let (_dir, store) = store();
        let dashboard = Dashboard::new(store).with_refresh_seconds(10);
        let base = spawn_server(&dashboard).await;
        let client = reqwest::Client::new();

        // Unknown POST path -> 404.
        let response = client.post(format!("{base}/nope")).send().await.unwrap();
        assert_eq!(response.status(), 404);

        // /api/cleanup/run with a query string -> 400 cleanup envelope.
        let response = client
            .post(format!("{base}/api/cleanup/run?force=1"))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 400);
        assert_eq!(response.json::<Value>().await.unwrap()["service"], "error");

        // Missing X-Stado-Action -> 403 cleanup envelope.
        let response = client
            .post(format!("{base}/api/cleanup/run"))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 403);
        assert_eq!(response.json::<Value>().await.unwrap()["service"], "error");

        // A non-empty body -> 400 (parameterless endpoint).
        let response = client
            .post(format!("{base}/api/cleanup/run"))
            .header("X-Stado-Action", "cleanup")
            .body("{}")
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 400);
    }

    #[test]
    fn trusted_request_host_matrix() {
        let _guard = crate::testutil::GLOBAL_STATE_LOCK.blocking_lock();
        let _env = EnvGuard::unset(&["STADO_DEPLOYMENT_ID"]);
        // IPs pass; DNS names fail without a deployment.
        assert!(trusted_request_host(Some("127.0.0.1"), None));
        assert!(trusted_request_host(Some("127.0.0.1:8765"), None));
        assert!(trusted_request_host(Some("[::1]:8765"), None));
        assert!(!trusted_request_host(Some("evil.example.com"), None));
        assert!(!trusted_request_host(Some("evil.example.com:8765"), None));
        assert!(!trusted_request_host(None, None));
        assert!(!trusted_request_host(Some(""), None));
        // userinfo / path / query are hard rejects even for IPs.
        assert!(!trusted_request_host(Some("u:p@127.0.0.1"), None));
        assert!(!trusted_request_host(Some("127.0.0.1/path"), None));
        assert!(!trusted_request_host(Some("127.0.0.1?q=1"), None));
        // Bad port on an IP: ValueError -> DNS branch -> false here.
        assert!(!trusted_request_host(Some("127.0.0.1:notaport"), None));
        assert!(!trusted_request_host(Some("127.0.0.1:99999"), None));
    }

    #[test]
    fn trusted_request_host_reverse_proxy_branch() {
        let _guard = crate::testutil::GLOBAL_STATE_LOCK.blocking_lock();
        let _env = EnvGuard::set(&[("STADO_DEPLOYMENT_ID", "dep-1")]);
        // DNS names pass only with the deployment set AND https forwarding.
        assert!(trusted_request_host(
            Some("dashboard.example.com"),
            Some("https")
        ));
        assert!(!trusted_request_host(
            Some("dashboard.example.com"),
            Some("http")
        ));
        assert!(!trusted_request_host(Some("dashboard.example.com"), None));
        // IPs still pass regardless.
        assert!(trusted_request_host(Some("10.0.0.1"), None));
    }

    #[test]
    fn parse_qs_decodes_and_skips_blank_values() {
        let pairs = parse_qs("ref=a%2Fb%3Ac&empty=&plus=a+b");
        let map: BTreeMap<_, _> = pairs.into_iter().collect();
        assert_eq!(map["ref"], "a/b:c");
        assert_eq!(map["plus"], "a b");
        assert_eq!(map["empty"], "");
    }
}
