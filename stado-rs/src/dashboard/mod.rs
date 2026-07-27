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
//! GET/PUT/DELETE /api/object?uri=stado://... - product object data plane
//! GET /api/object/list?namespace=...&prefix=... - product object listing
//! GET /api/object/stat?uri=stado://... - product object metadata
//! GET /api/release/object?uri=stado://releases/... - public software release download
//! POST /api/machine/submit - submit a canonical machine request
//! GET /api/machine/status?job_id=... - read canonical machine status
//! POST /api/machine/cancel?job_id=... - durably cancel a machine job
//! GET /api/service/status?name=... - read one managed service's beacon status
//! POST /api/service/restart?name=... - restart one managed service on every declared host
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

use std::io::Write;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

use crate::artifacts::registry::{ArtifactRegistry, RegistryError as ArtifactRegistryError};
use crate::artifacts_models::ArtifactRef;
use crate::config;
use crate::deploy::{host_channel, production_runner, service};
use crate::machine::{MachineError, MachineFacade};
use crate::models::isoformat_utc;
use crate::providers::local::disk_cleanup::{
    read_cleanup_state, run_cleanup_once, sanitize_cleanup_report,
};
use crate::queue::submit::json_dumps_sorted_compact;
use crate::queue::{python_json_dumps, JobStorage, StorageError};
use crate::targets;

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

    /// Start the refresh daemon and serve HTTP on loopback. This server does
    /// not terminate TLS, so binding it to a non-loopback interface would
    /// expose bearer-authenticated routes over plaintext. Production ingress
    /// must terminate TLS in a reverse proxy and forward to this listener.
    pub async fn serve_with(&self, host: &str, port: u16) -> Result<(), DashboardError> {
        let listener = TcpListener::bind((host, port)).await?;
        let local_addr = listener.local_addr()?;
        if !local_addr.ip().is_loopback() {
            return Err(DashboardError::Other(format!(
                "refusing plaintext dashboard bind on non-loopback address {local_addr}; terminate TLS in a loopback reverse proxy"
            )));
        }
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
        eprintln!("[dashboard] listening on http://{local_addr}");
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
        let Some(mut request) = read_request(&mut stream).await? else {
            return Ok(());
        };
        let is_object_put = request.method == "PUT" && request.path.starts_with("/api/object?");
        if is_object_put {
            if let Some(response) = self.object_put_preflight(&request).await {
                stream.write_all(&response.bytes).await?;
                return stream.shutdown().await;
            }
        }
        if request.body.len() < request.content_length {
            let received = request.body.len();
            request.body.resize(request.content_length, u8::default());
            stream.read_exact(&mut request.body[received..]).await?;
        }
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
            "PUT" => self.do_put(request).await,
            "DELETE" => self.do_delete(request).await,
            // Python BaseHTTPRequestHandler: 501 Unsupported method.
            _ => empty_response(501, "Not Implemented"),
        }
    }
    async fn object_put_preflight(&self, request: &Request) -> Option<Response> {
        if !trusted_request_host(request.header("host"), request.header("x-forwarded-proto")) {
            return Some(send_json(
                http_status("403"),
                &json!({"error": "forbidden"}),
            ));
        }
        let path = request.path.split('?').next().unwrap_or("");
        if path != "/api/object" {
            return Some(empty_response(http_status("404"), "Not Found"));
        }
        if !authorized(request, "object:write").await {
            return Some(send_json(
                http_status("401"),
                &json!({"error": "unauthorized"}),
            ));
        }
        None
    }

    async fn do_get(&self, request: &Request) -> Response {
        let path_no_query = request.path.split('?').next().unwrap_or("");
        let control_route =
            path_no_query == "/api/machine/status" || path_no_query == "/api/service/status";
        if !trusted_request_host(request.header("host"), request.header("x-forwarded-proto")) {
            return if control_route {
                send_json(http_status("403"), &json!({"error": "forbidden"}))
            } else {
                cleanup_failure(http_status("403"))
            };
        }
        if path_no_query == "/healthz" || path_no_query == "/livez" {
            return send_json(http_status("200"), &json!({"ok": true}));
        }
        let release_object_route = path_no_query == "/api/release/object";
        let object_route = path_no_query == "/api/object"
            || path_no_query == "/api/object/list"
            || path_no_query == "/api/object/stat";
        if !release_object_route {
            let permission = if path_no_query == "/api/machine/status" {
                "machine:status"
            } else if path_no_query == "/api/service/status" {
                "service:status"
            } else if object_route {
                "object:read"
            } else {
                "view"
            };
            if !authorized(request, permission).await {
                return send_json(http_status("401"), &json!({"error": "unauthorized"}));
            }
        }
        match self.get_routes(request).await {
            Ok(response) => response,
            // Python: a failing /api/cleanup.json answers the safe cleanup
            // envelope; every other route answers 500 "dashboard error".
            Err(_) if request.path == "/api/cleanup.json" => {
                cleanup_failure(http_status("500"))
            }
            Err(_) => Response::text(
                http_status("500"),
                "Internal Server Error",
                "dashboard error",
            ),
        }
    }

    async fn get_routes(&self, request: &Request) -> Result<Response, DashboardError> {
        let state = self.snapshot();
        let (path, query) = match request.path.split_once('?') {
            Some((path, query)) => (path, query),
            None => (request.path.as_str(), ""),
        };
        if path == "/api/release/object" {
            let object = match object_from_query(query) {
                Ok(object) => object,
                Err(response) => return Ok(response),
            };
            if object.namespace() != "releases" {
                return Ok(send_json(
                    http_status("403"),
                    &json!({"error": "only stado://releases software artifacts are publicly readable"}),
                ));
            }
            return self.get_object(request, query).await;
        }
        if path == "/api/object" {
            return self.get_object(request, query).await;
        }
        if path == "/api/object/list" {
            return self.list_objects(query).await;
        }
        if path == "/api/object/stat" {
            return self.stat_object(query).await;
        }
        if path == "/api/machine/status" {
            return Ok(self.get_machine_status(request, query).await);
        }
        if path == "/api/service/status" {
            return Ok(self.get_service_status(request, query).await);
        }
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

    async fn get_object(&self, request: &Request, query: &str) -> Result<Response, DashboardError> {
        let object = match object_from_query(query) {
            Ok(object) => object,
            Err(response) => return Ok(response),
        };
        let path = object.storage_path();
        let Some(bytes) = self.store.read_bytes(&path).await? else {
            return Ok(send_json(
                http_status("404"),
                &json!({"state": "absent", "uri": object.to_string()}),
            ));
        };
        let metadata = self
            .store
            .backend()
            .list_blobs_with_meta(&path)
            .await?
            .into_iter()
            .find(|blob| blob.name == path)
            .map(|blob| blob.metadata)
            .unwrap_or_default();
        let content_type = metadata
            .get("content-type")
            .map(String::as_str)
            .unwrap_or("application/octet-stream");
        if let Some(value) = request.header("range") {
            let Some((start, end)) = parse_byte_range(value, bytes.len()) else {
                return Ok(Response::new_with_headers(
                    http_status("416"),
                    "Range Not Satisfiable",
                    content_type,
                    b"",
                    &[("Content-Range", format!("bytes */{}", bytes.len()))],
                ));
            };
            return Ok(Response::new_with_headers(
                http_status("206"),
                "Partial Content",
                content_type,
                &bytes[start..=end],
                &[
                    ("Accept-Ranges", "bytes".to_string()),
                    (
                        "Content-Range",
                        format!("bytes {start}-{end}/{}", bytes.len()),
                    ),
                ],
            ));
        }
        Ok(Response::new_with_headers(
            http_status("200"),
            "OK",
            content_type,
            &bytes,
            &[("Accept-Ranges", "bytes".to_string())],
        ))
    }

    async fn list_objects(&self, query: &str) -> Result<Response, DashboardError> {
        let values = parse_qs(query);
        let namespace = query_value(&values, "namespace").unwrap_or_default();
        let prefix = query_value(&values, "prefix").unwrap_or_default();
        if namespace.is_empty() {
            return Ok(send_json(
                http_status("400"),
                &json!({"error": "namespace is required"}),
            ));
        }
        let storage_prefix = crate::object_store::ObjectRef::namespace_prefix(&namespace, &prefix)?;
        let objects = self
            .store
            .backend()
            .list_blobs_with_meta(&storage_prefix)
            .await?;
        let mut response = Vec::with_capacity(objects.len());
        for blob in objects {
            let object = crate::object_store::ObjectRef::from_storage_path(&blob.name)?;
            response.push(json!({
                "uri": object.to_string(),
                "namespace": object.namespace(),
                "key": object.key(),
                "size": blob.size,
                "updated_at": blob.updated.map(isoformat_utc),
                "metadata": blob.metadata,
            }));
        }
        Ok(send_json(http_status("200"), &json!({"objects": response})))
    }

    async fn stat_object(&self, query: &str) -> Result<Response, DashboardError> {
        let object = match object_from_query(query) {
            Ok(object) => object,
            Err(response) => return Ok(response),
        };
        let path = object.storage_path();
        let blob = self
            .store
            .backend()
            .list_blobs_with_meta(&path)
            .await?
            .into_iter()
            .find(|blob| blob.name == path);
        Ok(match blob {
            Some(blob) => send_json(
                http_status("200"),
                &json!({
                    "state": "present",
                    "uri": object.to_string(),
                    "size": blob.size,
                    "updated_at": blob.updated.map(isoformat_utc),
                    "metadata": blob.metadata,
                }),
            ),
            None => send_json(
                http_status("404"),
                &json!({"state": "absent", "uri": object.to_string()}),
            ),
        })
    }

    fn machine_facade(&self) -> MachineFacade {
        MachineFacade::with_store(
            self.store.clone(),
            self.store.bucket_name().to_string(),
        )
    }

    async fn get_machine_status(&self, request: &Request, query: &str) -> Response {
        if request.content_length != usize::default() || !request.body.is_empty() {
            return invalid_machine_request("machine status does not accept a request body");
        }
        let job_id = match machine_job_id(query) {
            Ok(job_id) => job_id,
            Err(response) => return response,
        };
        machine_result_response(self.machine_facade().status(job_id).await)
    }

    async fn post_machine_submit(&self, request: &Request) -> Response {
        let content_type = request
            .header("content-type")
            .unwrap_or("")
            .split(';')
            .next()
            .unwrap_or("")
            .trim();
        if request.path != "/api/machine/submit"
            || content_type != "application/json"
            || request.header("transfer-encoding").is_some()
            || request.header("content-length").is_none()
            || request.content_length != request.body.len()
            || request.body.len() > MAX_HEAD_BYTES
        {
            return invalid_machine_request("invalid JSON request framing");
        }
        let payload: Value = match serde_json::from_slice(&request.body) {
            Ok(payload) => payload,
            Err(error) => {
                return invalid_machine_request(format!("cannot read request JSON: {error}"))
            }
        };
        if let Err(error) = validate_remote_machine_request(&payload) {
            return machine_result_response(Err(error));
        }
        machine_result_response(self.machine_facade().submit_request(&payload).await)
    }

    async fn post_machine_cancel(&self, request: &Request, query: &str) -> Response {
        if request.header("transfer-encoding").is_some()
            || request.content_length != usize::default()
            || !request.body.is_empty()
        {
            return invalid_machine_request("machine cancel does not accept a request body");
        }
        let job_id = match machine_job_id(query) {
            Ok(job_id) => job_id,
            Err(response) => return response,
        };
        machine_result_response(self.machine_facade().cancel_job(job_id).await)
    }

    async fn get_service_status(&self, request: &Request, query: &str) -> Response {
        if request.content_length != usize::default() || !request.body.is_empty() {
            return invalid_service_request("service status does not accept a request body");
        }
        let name = match service_name(query) {
            Ok(name) => name,
            Err(response) => return response,
        };
        let store = match service_beacon_store().await {
            Ok(store) => store,
            Err(message) => {
                return service_failure(
                    http_status("503"),
                    "SERVICE_STATUS_FAILED",
                    message,
                    true,
                )
            }
        };
        let rows = match service::find_services(&store, name).await {
            Ok(rows) => rows,
            Err(error) => {
                return service_failure(
                    http_status("503"),
                    "SERVICE_STATUS_FAILED",
                    error.to_string(),
                    true,
                )
            }
        };
        if rows.is_empty() {
            return service_failure(
                http_status("404"),
                "NOT_FOUND",
                format!("no registry-managed service named {name}"),
                false,
            );
        }
        service_success(Value::Array(
            rows.iter().map(service::ServiceStatus::to_json).collect(),
        ))
    }

    async fn post_service_restart(&self, request: &Request, query: &str) -> Response {
        if request.header("transfer-encoding").is_some()
            || request.content_length != usize::default()
            || !request.body.is_empty()
        {
            return invalid_service_request("service restart does not accept a request body");
        }
        let name = match service_name(query) {
            Ok(name) => name,
            Err(response) => return response,
        };
        let services = match declared_services_matching(name).await {
            Ok(services) => services,
            Err(message) => {
                return service_failure(
                    http_status("503"),
                    "SERVICE_RESTART_FAILED",
                    message,
                    true,
                )
            }
        };
        if services.is_empty() {
            return service_failure(
                http_status("404"),
                "NOT_FOUND",
                format!("no registry-managed service named {name}"),
                false,
            );
        }
        let runner = production_runner();
        let mut result = Vec::with_capacity(services.len());
        let mut failures = Vec::new();
        for declared in &services {
            let target = match host_channel::canonical_target(&declared.host).await {
                Ok(target) => target,
                Err(error) => {
                    return service_failure(
                        http_status("503"),
                        "SERVICE_RESTART_FAILED",
                        error.to_string(),
                        true,
                    )
                }
            };
            let report = match service::restart_service(&target, declared, &runner).await {
                Ok(report) => report,
                Err(error) => {
                    return service_failure(
                        http_status("503"),
                        "SERVICE_RESTART_FAILED",
                        error.to_string(),
                        true,
                    )
                }
            };
            if !report.succeeded("restarted") {
                failures.push(format!("{}: {}", declared.host, report.failure()));
            }
            let mut entry = report.to_json();
            entry["host"] = Value::from(declared.host.clone());
            result.push(entry);
        }
        if !failures.is_empty() {
            return service_failure(
                http_status("503"),
                "SERVICE_RESTART_FAILED",
                format!("restart failed on {}", failures.join("; ")),
                true,
            );
        }
        service_success(Value::Array(result))
    }

    async fn put_object(&self, request: &Request, query: &str) -> Result<Response, DashboardError> {
        let object = match object_from_query(query) {
            Ok(object) => object,
            Err(response) => return Ok(response),
        };
        let values = parse_qs(query);
        let if_absent = query_value(&values, "if_absent").as_deref() == Some("true");
        let path = object.storage_path();
        if if_absent {
            let mut source = tempfile::NamedTempFile::new()?;
            source.write_all(&request.body)?;
            if !self
                .store
                .upload_file_if_absent(&path, source.path())
                .await?
            {
                return Ok(send_json(
                    http_status("409"),
                    &json!({"error": "object exists", "uri": object.to_string()}),
                ));
            }
        } else {
            self.store.upload_bytes(&path, &request.body).await?;
        }
        let content_type = request
            .header("content-type")
            .unwrap_or("application/octet-stream")
            .to_string();
        let metadata = crate::object_store::metadata(&object, &content_type);
        self.store.backend().set_metadata(&path, &metadata).await?;
        let landed = self.store.backend().list_blobs_with_meta(&path).await?;
        let Some(blob) = landed.into_iter().find(|blob| blob.name == path) else {
            return Err(DashboardError::Other(format!(
                "object metadata verification could not find {object}"
            )));
        };
        if metadata
            .iter()
            .filter(|(_, value)| !value.is_empty())
            .any(|(key, value)| blob.metadata.get(key) != Some(value))
        {
            return Err(DashboardError::Other(format!(
                "object metadata verification failed for {object}"
            )));
        }
        Ok(send_json(
            http_status("200"),
            &json!({
                "state": "stored",
                "uri": object.to_string(),
                "content_type": content_type,
            }),
        ))
    }

    async fn do_post(&self, request: &Request) -> Response {
        let (path, query) = request
            .path
            .split_once('?')
            .unwrap_or((request.path.as_str(), ""));
        let control_route = path == "/api/machine/submit"
            || path == "/api/machine/cancel"
            || path == "/api/service/restart";
        if !trusted_request_host(request.header("host"), request.header("x-forwarded-proto")) {
            return if control_route {
                send_json(http_status("403"), &json!({"error": "forbidden"}))
            } else {
                cleanup_failure(http_status("403"))
            };
        }
        if control_route {
            let permission = match path {
                "/api/machine/submit" => "machine:submit",
                "/api/machine/cancel" => "machine:cancel",
                _ => "service:restart",
            };
            if !authorized(request, permission).await {
                return send_json(http_status("401"), &json!({"error": "unauthorized"}));
            }
            return match path {
                "/api/machine/submit" => self.post_machine_submit(request).await,
                "/api/machine/cancel" => self.post_machine_cancel(request, query).await,
                _ => self.post_service_restart(request, query).await,
            };
        }
        if !authorized(request, "operate").await {
            return send_json(http_status("401"), &json!({"error": "unauthorized"}));
        }
        if path == "/api/registry/policy" {
            return self.post_registry_policy(request).await;
        }
        if request.path != "/api/cleanup/run" {
            return if path == "/api/cleanup/run" {
                cleanup_failure(http_status("400"))
            } else {
                empty_response(http_status("404"), "Not Found")
            };
        }
        match self.post_cleanup_run(request).await {
            Ok(response) => response,
            Err(_) => cleanup_failure(http_status("500")),
        }
    }

    async fn do_put(&self, request: &Request) -> Response {
        if !trusted_request_host(request.header("host"), request.header("x-forwarded-proto")) {
            return send_json(http_status("403"), &json!({"error": "forbidden"}));
        }
        let (path, query) = request
            .path
            .split_once('?')
            .unwrap_or((request.path.as_str(), ""));
        if path != "/api/object" {
            return empty_response(http_status("404"), "Not Found");
        }
        if !authorized(request, "object:write").await {
            return send_json(http_status("401"), &json!({"error": "unauthorized"}));
        }
        match self.put_object(request, query).await {
            Ok(response) => response,
            Err(error) => send_json(http_status("500"), &json!({"error": error.to_string()})),
        }
    }

    async fn do_delete(&self, request: &Request) -> Response {
        if !trusted_request_host(request.header("host"), request.header("x-forwarded-proto")) {
            return send_json(http_status("403"), &json!({"error": "forbidden"}));
        }
        let (path, query) = request
            .path
            .split_once('?')
            .unwrap_or((request.path.as_str(), ""));
        if path != "/api/object" {
            return empty_response(http_status("404"), "Not Found");
        }
        if !authorized(request, "object:write").await {
            return send_json(http_status("401"), &json!({"error": "unauthorized"}));
        }
        match object_from_query(query) {
            Ok(object) => {
                let result = self.store.delete_blob(&object.storage_path()).await;
                match result {
                    Ok(()) => send_json(
                        http_status("200"),
                        &json!({"state": "absent", "uri": object.to_string()}),
                    ),
                    Err(error) => {
                        send_json(http_status("500"), &json!({"error": error.to_string()}))
                    }
                }
            }
            Err(response) => response,
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

static MACHINE_JOB_ID_RE: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
    regex::Regex::new(r"^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$")
        .expect("static machine job ID regex compiles")
});


struct Request {
    method: String,
    /// Raw request target including the query string (Python `self.path`).
    path: String,
    /// Lowercased names with trimmed values.
    headers: Vec<(String, String)>,
    content_length: usize,
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
            if name == "transfer-encoding" {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "Transfer-Encoding is unsupported",
                ));
            }
            if name == "content-length" && headers.iter().any(|(key, _)| key == &name) {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "duplicate Content-Length",
                ));
            }
            headers.push((name, value.trim().to_string()));
        }
    }
    let body_start = head_end + b"\r\n\r\n".len();
    let content_length_header = headers
        .iter()
        .find(|(name, _)| name == "content-length")
        .map(|(_, value)| value.as_str());
    let object_put = method == "PUT" && path.starts_with("/api/object?");
    let content_length = match content_length_header {
        Some(value) => value.parse::<usize>().map_err(|_| {
            std::io::Error::new(std::io::ErrorKind::InvalidData, "invalid Content-Length")
        })?,
        None if object_put => {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "object PUT requires Content-Length",
            ));
        }
        None => usize::default(),
    };
    let max_body_bytes = if object_put {
        crate::object_store::max_object_bytes()
    } else {
        MAX_HEAD_BYTES
    };
    if content_length > max_body_bytes {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "HTTP request body too large",
        ));
    }
    let available = buf.len().saturating_sub(body_start).min(content_length);
    let mut body = Vec::with_capacity(available);
    body.extend_from_slice(&buf[body_start..body_start + available]);
    Ok(Some(Request {
        method,
        path,
        headers,
        content_length,
        body,
    }))
}

struct Response {
    status: u16,
    bytes: Vec<u8>,
}

impl Response {
    fn new(status: u16, reason: &str, content_type: &str, body: &[u8]) -> Self {
        Self::new_with_headers(status, reason, content_type, body, &[])
    }

    fn new_with_headers(
        status: u16,
        reason: &str,
        content_type: &str,
        body: &[u8],
        headers: &[(&str, String)],
    ) -> Self {
        let mut head = format!(
            "HTTP/1.1 {status} {reason}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\n",
            body.len()
        );
        for (name, value) in headers {
            head.push_str(name);
            head.push_str(": ");
            head.push_str(value);
            head.push_str("\r\n");
        }
        head.push_str("Connection: close\r\n\r\n");
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

fn parse_byte_range(value: &str, length: usize) -> Option<(usize, usize)> {
    if length == usize::default() {
        return None;
    }
    let value = value.strip_prefix("bytes=")?;
    if value.contains(',') {
        return None;
    }
    let (start, end) = value.split_once('-')?;
    let start = start.parse::<usize>().ok()?;
    if start >= length {
        return None;
    }
    let last = length.saturating_sub(usize::from(true));
    let end = if end.is_empty() {
        last
    } else {
        end.parse::<usize>().ok()?.min(last)
    };
    (start <= end).then_some((start, end))
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

fn machine_result_response(result: Result<Value, MachineError>) -> Response {
    match result {
        Ok(result) => send_json(http_status("200"), &json!({"ok": true, "result": result})),
        Err(error) => {
            let status = match error.code.as_str() {
                "INVALID_REQUEST" | "INVALID_SOURCE_ARCHIVE" => http_status("400"),
                "NOT_FOUND" => http_status("404"),
                "IDEMPOTENCY_CONFLICT" => http_status("409"),
                _ if error.retryable => http_status("503"),
                _ => http_status("500"),
            };
            send_json(
                status,
                &json!({
                    "ok": false,
                    "error": {
                        "code": error.code,
                        "message": error.message,
                        "retryable": error.retryable,
                    },
                }),
            )
        }
    }
}

fn invalid_machine_request(message: impl Into<String>) -> Response {
    machine_result_response(Err(MachineError::new("INVALID_REQUEST", message)))
}

fn machine_job_id(query: &str) -> Result<&str, Response> {
    let invalid = || {
        invalid_machine_request(
            "query must contain exactly one path-safe job_id parameter",
        )
    };
    if query.is_empty() || query.contains('&') {
        return Err(invalid());
    }
    let Some((name, job_id)) = query.split_once('=') else {
        return Err(invalid());
    };
    if name != "job_id" || !MACHINE_JOB_ID_RE.is_match(job_id) {
        return Err(invalid());
    }
    Ok(job_id)
}

fn service_name(query: &str) -> Result<&str, Response> {
    let invalid = || {
        invalid_service_request(
            "query must contain exactly one lowercase, path-safe name parameter",
        )
    };
    if query.is_empty() || query.contains('&') {
        return Err(invalid());
    }
    let Some((key, name)) = query.split_once('=') else {
        return Err(invalid());
    };
    if key != "name" || service::validate_service_name(name).is_err() {
        return Err(invalid());
    }
    Ok(name)
}

async fn service_beacon_store() -> Result<JobStorage, String> {
    let bucket = targets::GCS_REGISTRY_URI
        .split_once("//")
        .map(|(_, rest)| rest.split('/').next().unwrap_or_default())
        .unwrap_or_default();
    JobStorage::with_bucket(bucket)
        .await
        .map_err(|error| error.to_string())
}

async fn declared_services_matching(
    name: &str,
) -> Result<Vec<service::ManagedService>, String> {
    let registry = targets::fetch_registry_remote()
        .await
        .map_err(|error| error.to_string())?;
    let mut found = Vec::new();
    for target in registry.local_targets() {
        found.extend(
            service::declared_services(target)
                .into_iter()
                .filter(|declared| declared.matches(name)),
        );
    }
    Ok(found)
}

fn service_success(result: Value) -> Response {
    send_json(http_status("200"), &json!({"ok": true, "result": result}))
}

fn service_failure(
    status: u16,
    code: &str,
    message: impl Into<String>,
    retryable: bool,
) -> Response {
    send_json(
        status,
        &json!({
            "ok": false,
            "error": {
                "code": code,
                "message": message.into(),
                "retryable": retryable,
            },
        }),
    )
}

fn invalid_service_request(message: impl Into<String>) -> Response {
    service_failure(
        http_status("400"),
        "INVALID_REQUEST",
        message,
        false,
    )
}

fn validate_remote_machine_request(request: &Value) -> Result<(), MachineError> {
    let Some(request) = request.as_object() else {
        return Ok(());
    };
    if request.contains_key("source_archive_path") {
        return Err(MachineError::new(
            "INVALID_REQUEST",
            "source_archive_path is not accepted by the remote machine API; upload through the object API and declare a stado:// input_object",
        ));
    }
    let Some(inputs) = request.get("input_objects").and_then(Value::as_object) else {
        return Ok(());
    };
    for value in inputs.values() {
        let Some(spec) = value.as_object() else {
            continue;
        };
        if spec
            .keys()
            .any(|key| !matches!(key.as_str(), "stado_uri" | "relative_path" | "sha256"))
        {
            return Err(MachineError::new(
                "INVALID_REQUEST",
                "input_objects entries accept only stado_uri, relative_path, and sha256",
            ));
        }
    }
    Ok(())
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

fn query_value(values: &[(String, String)], name: &str) -> Option<String> {
    values
        .iter()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value.clone())
}

fn object_from_query(query: &str) -> Result<crate::object_store::ObjectRef, Response> {
    let values = parse_qs(query);
    let uri = query_value(&values, "uri").unwrap_or_default();
    if uri.is_empty() {
        return Err(send_json(
            http_status("400"),
            &json!({"error": "uri is required"}),
        ));
    }
    crate::object_store::ObjectRef::parse(&uri)
        .map_err(|error| send_json(http_status("400"), &json!({"error": error.to_string()})))
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

/// Accept loopback Host values for direct local access. A deployment-bound
/// HTTPS reverse proxy may forward either DNS or IP Host values; because the
/// listener itself is loopback-only, `X-Forwarded-Proto` cannot be supplied
/// by external plaintext ingress.
fn trusted_request_host(value: Option<&str>, forwarded_proto: Option<&str>) -> bool {
    let Some(value) = value else { return false };
    if value.is_empty() {
        return false;
    }
    // Malformed authorities always fail closed.
    let proxy_https =
        !config::stado_deployment_id().is_empty() && forwarded_proto == Some("https");

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
                    return false;
                };
                (host, port)
            }
            // Unterminated bracket — urlsplit raises ValueError.
            None => return false,
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
            return false;
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
    if host.eq_ignore_ascii_case("localhost") {
        return true;
    }
    match host.parse::<std::net::IpAddr>() {
        Ok(address) => address.is_loopback() || proxy_https,
        Err(_) => proxy_https,
    }
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    let left = Sha256::digest(left);
    let right = Sha256::digest(right);
    let mut difference = u8::default();
    for (left, right) in left.iter().zip(right) {
        difference |= left ^ right;
    }
    difference == u8::default()
}

/// Validate a route-specific service token or the Wisent session/deployment
/// grant through Supabase RLS. Machine, managed-service, and object tokens are
/// isolated. Machine and managed-service routes always fail closed and never
/// fall back to another service token or Supabase.
async fn authorized(request: &Request, permission: &str) -> bool {
    let control_item = if permission.starts_with("machine:") {
        Some("stado-machine-api")
    } else if permission.starts_with("service:") {
        Some("stado-service-api")
    } else {
        None
    };
    if let Some(item) = control_item {
        let expected = match crate::skarbiec::read_string(item, "token").await {
            Ok(Some(value)) => value,
            Ok(None) | Err(_) => String::new(),
        };
        let authorization = request.header("authorization").unwrap_or("").trim();
        let supplied = authorization.strip_prefix("Bearer ").unwrap_or_default();
        return !expected.is_empty()
            && constant_time_eq(expected.as_bytes(), supplied.as_bytes());
    }
    let deployment_id = config::stado_deployment_id();
    if permission.starts_with("object:") {
        let expected = match crate::skarbiec::read_string("stado-object-api", "token").await {
            Ok(Some(value)) => value,
            Ok(None) | Err(_) => String::new(),
        };
        let authorization = request.header("authorization").unwrap_or("").trim();
        let supplied = authorization.strip_prefix("Bearer ").unwrap_or_default();
        if !expected.is_empty() && constant_time_eq(expected.as_bytes(), supplied.as_bytes()) {
            return true;
        }
    }
    if deployment_id.is_empty() {
        return false;
    }
    let supabase_url = std::env::var("SUPABASE_URL").unwrap_or_default();
    let supabase_url = supabase_url.trim_end_matches('/');
    let anon_key = match crate::skarbiec::read_string("stado-supabase", "anon_key").await {
        Ok(Some(value)) => value,
        Ok(None) | Err(_) => return false,
    };
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
