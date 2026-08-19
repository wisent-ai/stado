//! The Stado API listener: the authenticated object, release, machine,
//! service, host-health and enrollment HTTP surface. Port of
//! `stado/dashboard.py` (ThreadingHTTPServer) with the HTML operator
//! dashboard removed — the operator workspace is Stado Desktop, and this
//! listener serves no page.
//!
//! GET/PUT/DELETE /api/object?uri=stado://... - product object data plane
//! GET /api/object/list?namespace=...&prefix=... - product object listing
//! GET /api/object/stat?uri=stado://... - product object metadata
//! PUT /api/host-health?host=... - route-scoped authenticated host beacon publication
//! GET /api/release/object?uri=stado://releases/... - public software release download
//! POST /api/machine/submit - submit a canonical machine request
//! GET /api/machine/status?job_id=... - read canonical machine status
//! POST /api/machine/cancel?job_id=... - durably cancel a machine job
//! GET /api/service/status?name=... - read one managed service's beacon status
//! POST /api/service/restart?name=... - restart one managed service on every declared host
//! POST /api/rate-limit/consume - authenticated shared atomic rate-limit consume
//! POST /api/integration/enterprise/<action> - authenticated read-only fleet projection
//! GET /api/fleet/invite/key - invite-token-authenticated public channel key
//! POST /api/fleet/join - invite-token-authenticated pending enrollment request
//! GET /join.sh           - machine-side enrollment bootstrap script (public)
//! GET /healthz           - liveness (before auth, after the Host guard)
//! GET /livez             - Cloud Run liveness alias
//!
//! `--enrollment-only` narrows this listener to exactly three of the routes
//! above — `GET /join.sh`, `GET /api/fleet/invite/key`,
//! `POST /api/fleet/join` — and answers 404 to every other path and method
//! before authorization, the store or the vault is touched. That mode exists
//! so the enrollment routes can be published through a tunnel without
//! publishing the object, machine or service planes with them. See
//! `ENROLLMENT_ROUTES`.
//!
//! The application plane was extracted into the private `wisent-backend`
//! service; Stado keeps only the generic object plane. The product
//! integrations (Stripe, Resend, SendGrid, GitHub, HuggingFace, captcha
//! proxies) were extracted into the private `wisent-integrations` service.
//!
//! DEVIATIONS from Python (deliberate):
//! - Hand-rolled minimal HTTP/1.1 on `tokio::net::TcpListener`, one task per
//!   accepted connection (the ThreadingHTTPServer equivalent); the port
//!   spec forbids adding a web-framework dependency. Python's implicit
//!   `Server:`/`Date:` response headers and its error-page HTML bodies are
//!   not reproduced (status codes and JSON bodies match).

mod fleet_join;
mod integration;

use futures::stream::{FuturesUnordered, StreamExt};
use std::io::Write;
use std::sync::{Arc, RwLock};
use std::time::Duration;

use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

use crate::config;
use crate::deploy::{host_channel, production_runner, service};
use crate::machine::{MachineError, MachineFacade, SCHEMA_VERSION as MACHINE_SCHEMA_VERSION};
use crate::models::isoformat_utc;
use crate::queue::submit::json_dumps_sorted_compact;
use crate::queue::{JobStorage, StorageError};
use crate::rate_limit::{self, ConsumeRequest, RateLimitError, RateLimiter};
use crate::targets;

/// Dashboard serve failure.
#[derive(Debug, thiserror::Error)]
pub enum DashboardError {
    /// Storage failures from the data-plane routes.
    #[error(transparent)]
    Storage(#[from] StorageError),
    /// Listener/socket failures.
    #[error(transparent)]
    Io(#[from] std::io::Error),
    /// Port validation and other serve failures.
    #[error("{0}")]
    Other(String),
}

/// Skarbiec boundary verdicts, recorded once at startup: every gated route
/// reads them to answer 503 (instead of hanging on a vault) when its
/// authorization boundary is down.
#[derive(Clone, Copy, Default)]
struct BoundaryAvailability {
    object: bool,
    release: bool,
    machine: bool,
    service: bool,
    rate_limit_verifier: bool,
    rate_limit_state: bool,
    integration: bool,
}

impl BoundaryAvailability {
    fn json(self) -> Value {
        json!({
            "object": self.object,
            "release": self.release,
            "machine": self.machine,
            "service": self.service,
            "rate_limit_verifier": self.rate_limit_verifier,
            "rate_limit_state": self.rate_limit_state,
            "integration": self.integration,
        })
    }

    fn all_ready(self) -> bool {
        self.object
            && self.release
            && self.machine
            && self.service
            && self.rate_limit_verifier
            && self.rate_limit_state
            && self.integration
    }
}

/// The exact (method, path) pairs `--enrollment-only` serves.
///
/// This is an ALLOWLIST, and it must stay one. A denylist of the sensitive
/// surfaces (`/api/object`, `/api/machine/submit`, ...) goes stale the
/// moment somebody adds a route: the new route is published by
/// default, and the mistake is invisible until the wrong thing answers on a
/// public tunnel. With an allowlist a new route is unreachable in this mode
/// until it is named here on purpose, so forgetting fails closed.
const ENROLLMENT_ROUTES: [(&str, &str); 3] = [
    ("GET", "/join.sh"),
    ("GET", "/api/fleet/invite/key"),
    ("POST", "/api/fleet/join"),
];

/// The machine-side bootstrap script this build serves at `GET /join.sh`.
///
/// Published for `stado fleet ingress`, which verifies a newly opened tunnel
/// by fetching that route from the internet and comparing the bytes with what
/// the listener behind the tunnel would have served. Empty in a build whose
/// source tree had no `deploy/join.sh`, exactly as the route is.
pub fn join_script_source() -> &'static str {
    fleet_join::join_script_source()
}

/// The one body every refused request gets in `--enrollment-only` mode.
///
/// Uniform and mute on purpose: it names no route, no surface and no
/// credential, so a caller cannot learn from a refusal that an operator plane
/// exists elsewhere, nor tell "wrong method on a served path" from "path this
/// listener has never heard of".
const ENROLLMENT_REFUSAL: &[u8] = b"not found\n";

/// Whether `method`+`path` is one of the three enrollment pairs. The query
/// string is not part of the decision; the routes parse their own.
fn enrollment_route_allowed(method: &str, path: &str) -> bool {
    let path = path.split('?').next().unwrap_or("");
    ENROLLMENT_ROUTES
        .iter()
        .any(|(allowed_method, allowed_path)| *allowed_method == method && *allowed_path == path)
}

#[derive(Clone)]
pub struct Dashboard {
    store: JobStorage,
    rate_limiter: RateLimiter,
    boundaries: Arc<RwLock<BoundaryAvailability>>,
    /// Serve only [`ENROLLMENT_ROUTES`]; every other request is refused
    /// before authorization, before the store and before the vault.
    enrollment_only: bool,
}

impl Dashboard {
    /// Bind the listener to a storage facade.
    pub fn new(store: JobStorage) -> Self {
        Self {
            rate_limiter: RateLimiter::new(store.clone()),
            boundaries: Arc::new(RwLock::new(BoundaryAvailability::default())),
            store,
            enrollment_only: false,
        }
    }

    /// Serve only the enrollment routes (`stado dashboard
    /// --enrollment-only`), so this listener can be published through a
    /// tunnel without publishing anything else.
    pub fn with_enrollment_only(mut self, enrollment_only: bool) -> Self {
        self.enrollment_only = enrollment_only;
        self
    }

    /// Start the boundary checks and serve HTTP on loopback. This server does
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
        if self.enrollment_only {
            // Nothing below this branch is started, because nothing below it
            // is reachable in this mode:
            //
            // * the seven Skarbiec boundary verifiers only gate object,
            //   release, machine, service, rate-limit and integration routes,
            //   all of which are refused by the allowlist. Skipping them also
            //   means this listener needs no vault at all, which is the point:
            //   it can run where the operator plane cannot.
            //
            // `boundaries` therefore stays all-false; no served route reads
            // it.
            //
            // This log is the operator's only confirmation of what they are
            // about to publish, so it names every served pair verbatim.
            eprintln!("[dashboard] enrollment-only listener on http://{local_addr}");
            eprintln!(
                "[dashboard] this listener serves ONLY the enrollment routes; every other path and method answers 404:"
            );
            for (method, path) in ENROLLMENT_ROUTES {
                eprintln!("[dashboard]   {method} {path}");
            }
            eprintln!(
                "[dashboard] no object, machine, service, host-health or integration route is served here"
            );
            return self.serve_on(listener).await;
        }
        // Each boundary reads every item its policy names, and each read is a
        // gpg decryption in the broker. Seventeen object namespaces against a
        // real vault with a cold gpg-agent exceeded the previous 15s and the
        // object API then answered 503 to the entire fleet until someone
        // restarted it -- a cold agent is a slow start, not a broken grant.
        let startup_timeout = Duration::from_secs(
            std::env::var("WC_DASHBOARD_BOUNDARY_TIMEOUT_SECONDS")
                .ok()
                .and_then(|value| value.trim().parse::<u64>().ok())
                .filter(|seconds| *seconds > 0)
                .unwrap_or(90),
        );
        // Every verifier reads shared Skarbiec vault/audit state. Starting all
        // boundaries together can overwhelm the listener and fail the whole
        // control plane on a transient connection reset, so validate them in
        // deterministic order with an independent timeout per boundary.
        //
        // A verdict is also recorded once and never revisited, so one slow or
        // reset read shuts a boundary until somebody restarts the unit -- and
        // `object` shutting means `503 object authorization unavailable` for the
        // whole fleet. That happened four times in one afternoon, each time
        // cured by an identical retry, so the retry belongs here instead of in
        // the operator's hands.
        let attempts = std::env::var("WC_DASHBOARD_BOUNDARY_ATTEMPTS")
            .ok()
            .and_then(|value| value.trim().parse::<usize>().ok())
            .filter(|count| *count > 0)
            .unwrap_or(3);
        let retry_pause = Duration::from_secs(2);
        macro_rules! validate {
            ($name:literal, $call:expr) => {{
                let mut outcome = tokio::time::timeout(startup_timeout, $call).await;
                let mut attempt = 1;
                while attempt < attempts && !matches!(outcome, Ok(Ok(_))) {
                    eprintln!(
                        "[dashboard] {} boundary attempt {attempt} of {attempts} did not settle; retrying",
                        $name
                    );
                    tokio::time::sleep(retry_pause).await;
                    outcome = tokio::time::timeout(startup_timeout, $call).await;
                    attempt += 1;
                }
                outcome
            }};
        }
        let object = validate!(
            "object authorization",
            crate::skarbiec::validate_object_verifier()
        );
        let release = validate!(
            "release publication",
            crate::skarbiec::validate_release_verifier()
        );
        let machine = validate!(
            "machine authorization",
            crate::skarbiec::validate_machine_verifier()
        );
        let service = validate!(
            "service authorization",
            crate::skarbiec::validate_service_verifier()
        );
        let rate_verifier = validate!("rate-limit authorization", rate_limit::validate_verifier());
        let rate_state = validate!("rate-limit state", self.rate_limiter.restore());
        let integration = validate!("integration authorization", integration::validate_startup());
        // Only `object` used to report why it failed, so every other boundary
        // said "unavailable" and left the operator guessing which grant, item
        // set or endpoint was at fault. The verdict is useless without it.
        macro_rules! explain {
            ($outcome:expr, $name:literal) => {
                match &$outcome {
                    Ok(Err(error)) => {
                        eprintln!("[dashboard] {} boundary error: {error:?}", $name)
                    }
                    Err(error) => {
                        eprintln!("[dashboard] {} boundary timed out: {error:?}", $name)
                    }
                    Ok(Ok(_)) => {}
                }
            };
        }
        explain!(object, "object authorization");
        explain!(release, "release publication");
        explain!(machine, "machine authorization");
        explain!(service, "service authorization");
        explain!(rate_verifier, "rate-limit authorization");
        explain!(rate_state, "rate-limit state");
        explain!(integration, "integration authorization");
        let boundaries = BoundaryAvailability {
            object: matches!(object, Ok(Ok(_))),
            release: matches!(release, Ok(Ok(_))),
            machine: matches!(machine, Ok(Ok(_))),
            service: matches!(service, Ok(Ok(_))),
            rate_limit_verifier: matches!(rate_verifier, Ok(Ok(_))),
            rate_limit_state: matches!(rate_state, Ok(Ok(_))),
            integration: matches!(integration, Ok(Ok(()))),
        };
        for (ready, name) in [
            (boundaries.object, "object authorization"),
            (boundaries.release, "release publication"),
            (boundaries.machine, "machine authorization"),
            (boundaries.service, "service authorization"),
            (boundaries.rate_limit_verifier, "rate-limit authorization"),
            (boundaries.rate_limit_state, "rate-limit state"),
            (boundaries.integration, "integration authorization"),
        ] {
            if !ready {
                eprintln!("[dashboard] {name} boundary unavailable");
            }
        }
        *self
            .boundaries
            .write()
            .expect("dashboard boundary state lock") = boundaries;
        eprintln!("[dashboard] listening on http://{local_addr}");
        self.serve_on(listener).await
    }

    /// Accept loop on an already-bound listener (tests bind 127.0.0.1:0).
    /// One task per connection — the ThreadingHTTPServer equivalent.
    pub async fn serve_on(&self, listener: TcpListener) -> Result<(), DashboardError> {
        let mut connections = FuturesUnordered::new();
        loop {
            tokio::select! {
                accepted = listener.accept() => {
                    let (stream, _) = accepted?;
                    let dashboard = self.clone();
                    connections.push(async move {
                        if let Err(exc) = dashboard.handle_connection(stream).await {
                            eprintln!("[dashboard] connection error: {exc}");
                        }
                    });
                }
                _ = connections.next(), if !connections.is_empty() => {}
            }
        }
    }

    async fn handle_connection(&self, mut stream: TcpStream) -> std::io::Result<()> {
        // The peer is the reverse proxy's loopback address behind an HTTPS
        // ingress; the invite routes only ever use it as a rate-limit bucket.
        let peer = stream.peer_addr().ok().map(|address| address.ip());
        let Some(mut request) = read_request(&mut stream).await? else {
            return Ok(());
        };
        request.peer = peer;
        // The mode gate is the FIRST thing that looks at the request, ahead of
        // the object PUT preflight, ahead of the remainder of the body, ahead
        // of every Host check and authorization, and ahead of any store or
        // vault access. A refused request costs one method/path comparison and
        // this listener never reads anything on its behalf; the rest of the
        // declared body is deliberately left unread, since the connection
        // closes anyway.
        if self.enrollment_only && !enrollment_route_allowed(&request.method, &request.path) {
            let response = Response::new(
                http_status("404"),
                "Not Found",
                "text/plain; charset=utf-8",
                ENROLLMENT_REFUSAL,
            );
            eprintln!(
                "[dashboard] \"{} {} HTTP/1.1\" {} enrollment-only",
                request.method, request.path, response.status
            );
            stream.write_all(&response.bytes).await?;
            return stream.shutdown().await;
        }
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
        let path = request.path.split('?').next().unwrap_or("");
        if path.starts_with("/api/integration/") {
            if !self
                .trusted_request_host(request.header("host"), request.header("x-forwarded-proto"))
            {
                return send_json(http_status("403"), &json!({"error": "forbidden"}));
            }
            let available = self
                .boundaries
                .read()
                .expect("dashboard boundary state lock")
                .integration;
            return integration::handle(request, available, &self.store).await;
        }
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
        if !self.trusted_request_host(request.header("host"), request.header("x-forwarded-proto")) {
            return Some(send_json(
                http_status("403"),
                &json!({"error": "forbidden"}),
            ));
        }
        let (path, query) = request
            .path
            .split_once('?')
            .unwrap_or((request.path.as_str(), ""));
        if path != "/api/object" {
            return Some(empty_response(http_status("404"), "Not Found"));
        }
        let object = match object_from_query(query) {
            Ok(object) => object,
            Err(response) => return Some(response),
        };
        let boundary_ready = {
            let boundaries = self
                .boundaries
                .read()
                .expect("dashboard boundary state lock");
            if crate::object_store::release_policy_key(object.namespace(), object.key()).is_some() {
                boundaries.release
            } else {
                boundaries.object
            }
        };
        if !boundary_ready {
            return Some(send_json(
                http_status("503"),
                &json!({"error": "object authorization unavailable"}),
            ));
        }
        let authorized = if let Some(policy_key) =
            crate::object_store::release_policy_key(object.namespace(), object.key())
        {
            // A release object is only ever created, never replaced; the
            // client resolves its credential from the same routing function.
            let immutable = query_value(&parse_qs(query), "if_absent").as_deref() == Some("true");
            if object.namespace() == "releases" && !immutable {
                Ok(false)
            } else {
                authorize_release(request, &policy_key, false).await
            }
        } else {
            authorize_object(request, object.namespace(), object.key(), false, "put").await
        };
        match authorized {
            Ok(true) => {}
            Ok(false) => {
                return Some(send_json(
                    http_status("401"),
                    &json!({"error": "unauthorized or non-immutable release write"}),
                ))
            }
            Err(()) => {
                return Some(send_json(
                    http_status("503"),
                    &json!({"error": "object authorization unavailable"}),
                ))
            }
        }
        None
    }

    async fn do_get(&self, request: &Request) -> Response {
        let path_no_query = request.path.split('?').next().unwrap_or("");
        if !self.trusted_request_host(request.header("host"), request.header("x-forwarded-proto")) {
            if path_no_query == "/api/machine/status" {
                return machine_result_response(Err(MachineError::new("FORBIDDEN", "forbidden")));
            }
            return send_json(http_status("403"), &json!({"error": "forbidden"}));
        }
        if path_no_query == "/healthz" || path_no_query == "/livez" {
            let boundaries = *self
                .boundaries
                .read()
                .expect("dashboard boundary state lock");
            return send_json(
                http_status("200"),
                &json!({
                    "ok": true,
                    "degraded": !boundaries.all_ready(),
                    "boundaries": boundaries.json(),
                }),
            );
        }
        // Enrollment by invite. Both answer before any operator
        // authorization is consulted, and neither ever consults it: the
        // machine holds an invite code and nothing else, and a loopback
        // caller's implicit operator trust must not become a way in.
        if path_no_query == "/join.sh" {
            return fleet_join::join_script();
        }
        if path_no_query == "/api/fleet/invite/key" {
            return fleet_join::invite_key(&self.store, request).await;
        }
        let release_object_route = path_no_query == "/api/release/object";
        let object_route = path_no_query == "/api/object"
            || path_no_query == "/api/object/list"
            || path_no_query == "/api/object/stat";
        if object_route {
            let query = request
                .path
                .split_once('?')
                .map(|(_, query)| query)
                .unwrap_or("");
            let scope = if path_no_query == "/api/object/list" {
                object_list_from_query(query).map(|(namespace, prefix)| (namespace, prefix, true))
            } else {
                object_from_query(query).map(|object| {
                    (
                        object.namespace().to_string(),
                        object.key().to_string(),
                        false,
                    )
                })
            };
            let (namespace, key_or_prefix, list) = match scope {
                Ok(scope) => scope,
                Err(response) => return response,
            };
            let boundary_ready = {
                let boundaries = self
                    .boundaries
                    .read()
                    .expect("dashboard boundary state lock");
                if crate::object_store::release_policy_key(&namespace, &key_or_prefix).is_some() {
                    boundaries.release
                } else {
                    boundaries.object
                }
            };
            if !boundary_ready {
                return send_json(
                    http_status("503"),
                    &json!({"error": "object authorization unavailable"}),
                );
            }
            let action = if list {
                "list"
            } else if path_no_query == "/api/object/stat" {
                "stat"
            } else {
                "get"
            };
            let authorized = if let Some(policy_key) =
                crate::object_store::release_policy_key(&namespace, &key_or_prefix)
            {
                // A catalog object is addressed exactly, never listed as a prefix.
                let listing = list && namespace != "system";
                authorize_release(request, &policy_key, listing).await
            } else {
                authorize_object(request, &namespace, &key_or_prefix, list, action).await
            };
            match authorized {
                Ok(true) => {}
                Ok(false) => {
                    return send_json(http_status("401"), &json!({"error": "unauthorized"}))
                }
                Err(()) => {
                    return send_json(
                        http_status("503"),
                        &json!({"error": "object authorization unavailable"}),
                    )
                }
            }
        } else if !release_object_route {
            let boundaries = *self
                .boundaries
                .read()
                .expect("dashboard boundary state lock");
            if path_no_query == "/api/service/status" && !boundaries.service {
                return send_json(
                    http_status("503"),
                    &json!({"error": "service authorization unavailable"}),
                );
            }
            if path_no_query == "/api/machine/status" && !boundaries.machine {
                return machine_result_response(Err(MachineError::retryable(
                    "AUTH_UNAVAILABLE",
                    "machine authorization unavailable",
                )));
            }
            if path_no_query == "/api/service/status" {
                let query = request
                    .path
                    .split_once('?')
                    .map(|(_, query)| query)
                    .unwrap_or("");
                let service = match service_name(query) {
                    Ok(service) => service,
                    Err(response) => return response,
                };
                match authorize_service(request, service, "status").await {
                    Ok(true) => {}
                    Ok(false) => {
                        return send_json(http_status("401"), &json!({"error": "unauthorized"}))
                    }
                    Err(()) => {
                        return send_json(
                            http_status("503"),
                            &json!({"error": "service authorization unavailable"}),
                        )
                    }
                }
            }
        }
        match self.get_routes(request).await {
            Ok(response) => response,
            Err(_) => Response::text(
                http_status("500"),
                "Internal Server Error",
                "dashboard error",
            ),
        }
    }

    async fn get_routes(&self, request: &Request) -> Result<Response, DashboardError> {
        let (path, query) = match request.path.split_once('?') {
            Some((path, query)) => (path, query),
            None => (request.path.as_str(), ""),
        };
        if path == "/api/release/object" {
            // The fleet's public read-only release channel. The object API
            // daemon (com.wisent.always-on.stado-object-api) IS this server —
            // scripts/stado-object-api-mini-launcher.sh runs
            // `stado dashboard --bind 127.0.0.1 --port 8765` — so this route
            // is the store's own delivery endpoint, reached off-host through
            // the tailnet TLS terminator
            // (deploy/com.wisent.always-on.stado-tailnet-object-proxy.plist).
            // The response contract the readers parse
            // (cli::storage::get_release/stat_release, self_update,
            // deploy::bootstrap, deploy::host_release):
            //   200: the object bytes, Content-Type from object metadata
            //        (default application/octet-stream), Accept-Ranges: bytes
            //   206/416: byte-range answers from get_object
            //   400: {"error": ...} — missing or unparseable uri
            //   403: {"error": ...} — a namespace that is not `releases`
            //   404: {"state": "absent", "uri": ...} — the object is missing
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
        Ok(empty_response(404, "Not Found"))
    }

    async fn get_object(&self, request: &Request, query: &str) -> Result<Response, DashboardError> {
        let object = match object_from_query(query) {
            Ok(object) => object,
            Err(response) => return Ok(response),
        };
        let path = object.storage_path();
        let versioned = query_value(&parse_qs(query), "versioned").as_deref() == Some("true");
        let (bytes, version) = if versioned {
            let Some(value) = self.store.read_text_versioned(&path).await? else {
                return Ok(send_json(
                    http_status("404"),
                    &json!({"state": "absent", "uri": object.to_string()}),
                ));
            };
            (value.content.into_bytes(), Some(value.version))
        } else {
            let Some(bytes) = self.store.read_bytes(&path).await? else {
                return Ok(send_json(
                    http_status("404"),
                    &json!({"state": "absent", "uri": object.to_string()}),
                ));
            };
            (bytes, None)
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
        let mut headers = vec![("Accept-Ranges", "bytes".to_string())];
        if let Some(version) = version {
            headers.push(("X-Stado-Version", version));
        }
        Ok(Response::new_with_headers(
            http_status("200"),
            "OK",
            content_type,
            &bytes,
            &headers,
        ))
    }

    async fn list_objects(&self, query: &str) -> Result<Response, DashboardError> {
        let (namespace, requested_prefix) = match object_list_from_query(query) {
            Ok(scope) => scope,
            Err(response) => return Ok(response),
        };
        let prefix = if release_object_namespace(&namespace) {
            config::release_publisher_for_list(&requested_prefix).map(|(_, authorized)| authorized)
        } else {
            config::object_api_namespace(&namespace)
                .and_then(|policy| policy.authorized_list_prefix(&requested_prefix, "list"))
        };
        let Some(prefix) = prefix else {
            return Ok(send_json(
                http_status("401"),
                &json!({"error": "unauthorized"}),
            ));
        };
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
        MachineFacade::with_store(self.store.clone(), self.store.bucket_name().to_string())
    }

    async fn get_machine_status(&self, request: &Request, query: &str) -> Response {
        if request.content_length != usize::default() || !request.body.is_empty() {
            return invalid_machine_request("machine status does not accept a request body");
        }
        let client = match authenticate_machine_client(request, "status").await {
            Ok(Some(client)) => client,
            Ok(None) => {
                return machine_result_response(Err(MachineError::new(
                    "UNAUTHORIZED",
                    "unauthorized",
                )))
            }
            Err(()) => {
                return machine_result_response(Err(MachineError::retryable(
                    "AUTH_UNAVAILABLE",
                    "machine authorization unavailable",
                )))
            }
        };
        let job_id = match machine_job_id(query) {
            Ok(job_id) => job_id,
            Err(response) => return response,
        };
        let result = self.machine_facade().status(job_id).await;
        let target_allowed = result
            .as_ref()
            .ok()
            .and_then(machine_result_target)
            .is_some_and(|target| client.allows_target(target));
        if !target_allowed {
            return machine_result_response(Err(MachineError::new("UNAUTHORIZED", "unauthorized")));
        }
        machine_result_response(result)
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
        let mut payload: Value = match serde_json::from_slice(&request.body) {
            Ok(payload) => payload,
            Err(error) => {
                return invalid_machine_request(format!("cannot read request JSON: {error}"))
            }
        };
        if let Err(error) = validate_remote_machine_request(&payload) {
            return machine_result_response(Err(error));
        }
        let client = match authenticate_machine_client(request, "submit").await {
            Ok(Some(client)) => client,
            Ok(None) => {
                return machine_result_response(Err(MachineError::new(
                    "UNAUTHORIZED",
                    "unauthorized",
                )))
            }
            Err(()) => {
                return machine_result_response(Err(MachineError::retryable(
                    "AUTH_UNAVAILABLE",
                    "machine authorization unavailable",
                )))
            }
        };
        let requested = payload
            .get("provider")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let target = if requested.is_empty() {
            let [target] = client.targets() else {
                return machine_result_response(Err(MachineError::new(
                    "UNAUTHORIZED",
                    "unauthorized",
                )));
            };
            target.clone()
        } else if client.allows_target(requested) {
            requested.to_string()
        } else {
            return machine_result_response(Err(MachineError::new("UNAUTHORIZED", "unauthorized")));
        };
        let Some(object) = payload.as_object_mut() else {
            return invalid_machine_request("machine request must be an object");
        };
        object.insert("provider".to_string(), Value::String(target));
        object.insert("pin_to_provider".to_string(), Value::Bool(true));
        machine_result_response(self.machine_facade().submit_request(&payload).await)
    }

    async fn post_machine_cancel(&self, request: &Request, query: &str) -> Response {
        if request.header("transfer-encoding").is_some()
            || request.content_length != usize::default()
            || !request.body.is_empty()
        {
            return invalid_machine_request("machine cancel does not accept a request body");
        }
        let client = match authenticate_machine_client(request, "cancel").await {
            Ok(Some(client)) => client,
            Ok(None) => {
                return machine_result_response(Err(MachineError::new(
                    "UNAUTHORIZED",
                    "unauthorized",
                )))
            }
            Err(()) => {
                return machine_result_response(Err(MachineError::retryable(
                    "AUTH_UNAVAILABLE",
                    "machine authorization unavailable",
                )))
            }
        };
        let job_id = match machine_job_id(query) {
            Ok(job_id) => job_id,
            Err(response) => return response,
        };
        let status = self.machine_facade().status(job_id).await;
        let target_allowed = status
            .as_ref()
            .ok()
            .and_then(machine_result_target)
            .is_some_and(|target| client.allows_target(target));
        if !target_allowed {
            return machine_result_response(Err(MachineError::new("UNAUTHORIZED", "unauthorized")));
        }
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
                return service_failure(http_status("503"), "SERVICE_STATUS_FAILED", message, true)
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
                return service_failure(http_status("503"), "SERVICE_RESTART_FAILED", message, true)
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

    async fn put_object(
        &self,
        request: &Request,
        object: &crate::object_store::ObjectRef,
        query: &str,
    ) -> Result<Response, DashboardError> {
        let values = parse_qs(query);
        let if_absent = query_value(&values, "if_absent").as_deref() == Some("true");
        let if_version = query_value(&values, "if_version").filter(|value| !value.is_empty());
        let metadata_only = query_value(&values, "metadata_only").as_deref() == Some("true");
        let selected =
            usize::from(if_absent) + usize::from(if_version.is_some()) + usize::from(metadata_only);
        if selected > 1 {
            return Ok(send_json(
                http_status("400"),
                &json!({"error": "if_absent, if_version, and metadata_only are mutually exclusive"}),
            ));
        }
        let path = object.storage_path();
        if metadata_only {
            if !self.store.backend().exists(&path).await? {
                return Ok(send_json(
                    http_status("404"),
                    &json!({"state": "absent", "uri": object.to_string()}),
                ));
            }
            let metadata: std::collections::BTreeMap<String, String> =
                match serde_json::from_slice(&request.body) {
                    Ok(metadata) => metadata,
                    Err(error) => {
                        return Ok(send_json(
                            http_status("400"),
                            &json!({"error": format!("invalid metadata: {error}")}),
                        ))
                    }
                };
            self.store.backend().set_metadata(&path, &metadata).await?;
            return Ok(send_json(
                http_status("200"),
                &json!({"state": "metadata-updated", "uri": object.to_string()}),
            ));
        }
        if let Some(expected_version) = if_version {
            let content = match std::str::from_utf8(&request.body) {
                Ok(content) => content,
                Err(error) => {
                    return Ok(send_json(
                        http_status("400"),
                        &json!({"error": format!("conditional object writes require UTF-8: {error}")}),
                    ))
                }
            };
            let version = match self
                .store
                .compare_and_swap_text(&path, &expected_version, content)
                .await
            {
                Ok(version) => version,
                Err(StorageError::StorageConflict(_)) => {
                    return Ok(send_json(
                        http_status("409"),
                        &json!({"error": "object version changed", "uri": object.to_string()}),
                    ))
                }
                Err(StorageError::NotFound(_)) => {
                    return Ok(send_json(
                        http_status("404"),
                        &json!({"state": "absent", "uri": object.to_string()}),
                    ))
                }
                Err(error) => return Err(error.into()),
            };
            return Ok(send_json(
                http_status("200"),
                &json!({
                    "state": "stored",
                    "uri": object.to_string(),
                    "version": version,
                }),
            ));
        }
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
        let mut metadata = crate::object_store::metadata(object, &content_type);
        if let Some(raw) = request.header("x-stado-object-metadata") {
            let extra: std::collections::BTreeMap<String, String> = match serde_json::from_str(raw)
            {
                Ok(value) => value,
                Err(error) => {
                    return Ok(send_json(
                        http_status("400"),
                        &json!({"error": format!("invalid object metadata: {error}")}),
                    ))
                }
            };
            for (name, value) in extra {
                if !name.starts_with("stado-")
                    || metadata.contains_key(&name)
                    || value.is_empty()
                    || name.chars().any(char::is_control)
                    || value.chars().any(char::is_control)
                {
                    return Ok(send_json(
                        http_status("400"),
                        &json!({"error": "custom object metadata must use unique non-empty stado-* fields"}),
                    ));
                }
                metadata.insert(name, value);
            }
        }
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

    async fn post_rate_limit_consume(&self, request: &Request) -> Response {
        let content_type = request
            .header("content-type")
            .and_then(|value| value.split(';').next())
            .map(str::trim);
        if content_type != Some("application/json") {
            return send_json(
                http_status("415"),
                &json!({"error": "content-type must be application/json"}),
            );
        }
        let supplied = request
            .header("authorization")
            .and_then(|value| value.trim().strip_prefix("Bearer "))
            .unwrap_or_default();
        let client = match rate_limit::authenticate(supplied).await {
            Ok(Some(client)) => client,
            Ok(None) => return send_json(http_status("401"), &json!({"error": "unauthorized"})),
            Err(_) => {
                return send_json(
                    http_status("503"),
                    &json!({"error": "rate limiting unavailable"}),
                )
            }
        };
        let payload = match serde_json::from_slice::<ConsumeRequest>(&request.body) {
            Ok(payload) => payload,
            Err(_) => {
                return send_json(
                    http_status("400"),
                    &json!({"error": "invalid rate-limit request"}),
                )
            }
        };
        match self.rate_limiter.consume(client, &payload).await {
            Ok(response) => send_json(http_status("200"), &json!(response)),
            Err(RateLimitError::InvalidRequest(message)) => {
                send_json(http_status("400"), &json!({"error": message}))
            }
            Err(_) => send_json(
                http_status("503"),
                &json!({"error": "rate limiting unavailable"}),
            ),
        }
    }

    async fn do_post(&self, request: &Request) -> Response {
        let (path, query) = request
            .path
            .split_once('?')
            .unwrap_or((request.path.as_str(), ""));
        let control_route = path == "/api/machine/submit"
            || path == "/api/machine/cancel"
            || path == "/api/service/restart"
            || path == "/api/rate-limit/consume";
        if !self.trusted_request_host(request.header("host"), request.header("x-forwarded-proto")) {
            if matches!(path, "/api/machine/submit" | "/api/machine/cancel") {
                return machine_result_response(Err(MachineError::new("FORBIDDEN", "forbidden")));
            }
            return send_json(http_status("403"), &json!({"error": "forbidden"}));
        }
        // Enrollment by invite: authorized by the invite token alone, before
        // any operator authorization is reached, and never by it.
        if path == "/api/fleet/join" {
            return fleet_join::join(&self.store, request).await;
        }
        let boundaries = *self
            .boundaries
            .read()
            .expect("dashboard boundary state lock");
        let unavailable = (path == "/api/rate-limit/consume"
            && (!boundaries.rate_limit_verifier || !boundaries.rate_limit_state))
            || (matches!(path, "/api/machine/submit" | "/api/machine/cancel")
                && !boundaries.machine)
            || (path == "/api/service/restart" && !boundaries.service);
        if unavailable {
            if matches!(path, "/api/machine/submit" | "/api/machine/cancel") {
                return machine_result_response(Err(MachineError::retryable(
                    "AUTH_UNAVAILABLE",
                    "machine authorization unavailable",
                )));
            }
            return send_json(
                http_status("503"),
                &json!({"error": "authorization boundary unavailable"}),
            );
        }
        if path == "/api/rate-limit/consume" {
            return self.post_rate_limit_consume(request).await;
        }
        if control_route {
            if path == "/api/service/restart" {
                let service = match service_name(query) {
                    Ok(service) => service,
                    Err(response) => return response,
                };
                match authorize_service(request, service, "restart").await {
                    Ok(true) => {}
                    Ok(false) => {
                        return send_json(http_status("401"), &json!({"error": "unauthorized"}))
                    }
                    Err(()) => {
                        return send_json(
                            http_status("503"),
                            &json!({"error": "service authorization unavailable"}),
                        )
                    }
                }
                return self.post_service_restart(request, query).await;
            }
            return if path == "/api/machine/submit" {
                self.post_machine_submit(request).await
            } else {
                self.post_machine_cancel(request, query).await
            };
        }
        empty_response(http_status("404"), "Not Found")
    }

    async fn do_put(&self, request: &Request) -> Response {
        if !self.trusted_request_host(request.header("host"), request.header("x-forwarded-proto")) {
            return send_json(http_status("403"), &json!({"error": "forbidden"}));
        }
        let (path, query) = request
            .path
            .split_once('?')
            .unwrap_or((request.path.as_str(), ""));
        if path == "/api/host-health" {
            return self.put_host_health(request, query).await;
        }
        if path != "/api/object" {
            return empty_response(http_status("404"), "Not Found");
        }
        let object = match object_from_query(query) {
            Ok(object) => object,
            Err(response) => return response,
        };
        let boundary_ready = {
            let boundaries = self
                .boundaries
                .read()
                .expect("dashboard boundary state lock");
            if crate::object_store::release_policy_key(object.namespace(), object.key()).is_some() {
                boundaries.release
            } else {
                boundaries.object
            }
        };
        if !boundary_ready {
            return send_json(
                http_status("503"),
                &json!({"error": "object authorization unavailable"}),
            );
        }
        let authorized = if let Some(policy_key) =
            crate::object_store::release_policy_key(object.namespace(), object.key())
        {
            // A release object is only ever created, never replaced; the
            // client resolves its credential from the same routing function.
            let immutable = query_value(&parse_qs(query), "if_absent").as_deref() == Some("true");
            if object.namespace() == "releases" && !immutable {
                Ok(false)
            } else {
                authorize_release(request, &policy_key, false).await
            }
        } else {
            authorize_object(request, object.namespace(), object.key(), false, "put").await
        };
        match authorized {
            Ok(true) => {}
            Ok(false) => {
                return send_json(
                    http_status("401"),
                    &json!({"error": "unauthorized or non-immutable release write"}),
                )
            }
            Err(()) => {
                return send_json(
                    http_status("503"),
                    &json!({"error": "object authorization unavailable"}),
                )
            }
        }
        match self.put_object(request, &object, query).await {
            Ok(response) => response,
            Err(error) => send_json(http_status("500"), &json!({"error": error.to_string()})),
        }
    }

    async fn do_delete(&self, request: &Request) -> Response {
        if !self.trusted_request_host(request.header("host"), request.header("x-forwarded-proto")) {
            return send_json(http_status("403"), &json!({"error": "forbidden"}));
        }
        let (path, query) = request
            .path
            .split_once('?')
            .unwrap_or((request.path.as_str(), ""));
        if path != "/api/object" {
            return empty_response(http_status("404"), "Not Found");
        }
        let object = match object_from_query(query) {
            Ok(object) => object,
            Err(response) => return response,
        };
        if release_object_namespace(object.namespace()) {
            return send_json(
                http_status("403"),
                &json!({"error": "release objects are immutable and cannot be deleted"}),
            );
        }
        if !self
            .boundaries
            .read()
            .expect("dashboard boundary state lock")
            .object
        {
            return send_json(
                http_status("503"),
                &json!({"error": "object authorization unavailable"}),
            );
        }
        match authorize_object(request, object.namespace(), object.key(), false, "delete").await {
            Ok(true) => {}
            Ok(false) => return send_json(http_status("401"), &json!({"error": "unauthorized"})),
            Err(()) => {
                return send_json(
                    http_status("503"),
                    &json!({"error": "object authorization unavailable"}),
                )
            }
        }
        let result = self.store.delete_blob(&object.storage_path()).await;
        match result {
            Ok(()) => send_json(
                http_status("200"),
                &json!({"state": "absent", "uri": object.to_string()}),
            ),
            Err(error) => send_json(http_status("500"), &json!({"error": error.to_string()})),
        }
    }

    async fn put_host_health(&self, request: &Request, query: &str) -> Response {
        if !authorize_host_health(request).await {
            return send_json(http_status("401"), &json!({"error": "unauthorized"}));
        }
        let values = parse_qs(query);
        let host = match values.as_slice() {
            [(key, value)] if key == "host" => value.clone(),
            _ => {
                return send_json(
                    http_status("400"),
                    &json!({"error": "exactly one host query parameter is required"}),
                )
            }
        };
        if !valid_beacon_host(&host) {
            return send_json(
                http_status("400"),
                &json!({"error": "host must be a lowercase DNS label"}),
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
            || content_length != Some(request.body.len())
            || request.body.is_empty()
        {
            return send_json(
                http_status("400"),
                &json!({"error": "invalid JSON request framing"}),
            );
        }
        let payload: Value = match serde_json::from_slice(&request.body) {
            Ok(value) => value,
            Err(error) => {
                return send_json(
                    http_status("400"),
                    &json!({"error": format!("invalid JSON: {error}")}),
                )
            }
        };
        let Some(document) = payload.as_object() else {
            return send_json(
                http_status("400"),
                &json!({"error": "host beacon must be a JSON object"}),
            );
        };
        if document.get("host").and_then(Value::as_str) != Some(host.as_str())
            || document
                .get("reported_at")
                .and_then(Value::as_str)
                .is_none()
            || document.get("units").and_then(Value::as_object).is_none()
        {
            return send_json(
                http_status("400"),
                &json!({"error": "beacon host must match the query and reported_at/units are required"}),
            );
        }
        let path = crate::monitor::host_health::beacon_object_path(&host);
        match self.store.upload_bytes(&path, &request.body).await {
            Ok(()) => send_json(
                http_status("200"),
                &json!({"state": "stored", "host": host, "path": path}),
            ),
            Err(error) => send_json(http_status("500"), &json!({"error": error.to_string()})),
        }
    }

    fn deployment_id(&self) -> String {
        config::stado_deployment_id()
    }

    fn trusted_request_host(&self, value: Option<&str>, forwarded_proto: Option<&str>) -> bool {
        let reverse_proxy_enabled =
            config::dashboard_trust_https_proxy() || !self.deployment_id().is_empty();
        trusted_request_host(value, forwarded_proto, reverse_proxy_enabled)
    }
}

/// Python `serve(host=None, port=None)`: run the API listener. Blocks until
/// killed. Defaults from `config::dashboard_bind()` /
/// `config::dashboard_port()`; storage from `config::bucket()`.
///
/// `enrollment_only` narrows the listener to `ENROLLMENT_ROUTES` — the mode
/// that is safe to publish through a tunnel.
pub async fn serve(
    host: Option<&str>,
    port: Option<i64>,
    enrollment_only: bool,
) -> Result<(), DashboardError> {
    let host = host
        .map(str::to_string)
        .unwrap_or_else(|| config::dashboard_bind().to_string());
    let port = port.unwrap_or_else(config::dashboard_port);
    let port = u16::try_from(port)
        .map_err(|_| DashboardError::Other(format!("dashboard port out of range: {port}")))?;
    let store = JobStorage::new().await?;
    Dashboard::new(store)
        .with_enrollment_only(enrollment_only)
        .serve_with(&host, port)
        .await
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
    /// Connection peer, filled in by the accept path. `None` when the socket
    /// no longer has one to report.
    peer: Option<std::net::IpAddr>,
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
    let mut body = Vec::with_capacity(content_length);
    body.extend_from_slice(&buf[body_start..body_start + available]);
    if body.len() < content_length {
        let received = body.len();
        body.resize(content_length, 0);
        stream.read_exact(&mut body[received..]).await?;
    }
    Ok(Some(Request {
        method,
        path,
        headers,
        content_length,
        body,
        peer: None,
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
        Ok(result) => send_json(
            http_status("200"),
            &json!({"schema_version": MACHINE_SCHEMA_VERSION, "ok": true, "result": result}),
        ),
        Err(error) => {
            let status = match error.code.as_str() {
                "INVALID_REQUEST" | "INVALID_SOURCE_ARCHIVE" => http_status("400"),
                "NOT_FOUND" => http_status("404"),
                "IDEMPOTENCY_CONFLICT" => http_status("409"),
                "UNAUTHORIZED" => http_status("401"),
                "FORBIDDEN" => http_status("403"),
                _ if error.retryable => http_status("503"),
                _ => http_status("500"),
            };
            send_json(
                status,
                &json!({
                    "schema_version": MACHINE_SCHEMA_VERSION,
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
    let invalid =
        || invalid_machine_request("query must contain exactly one path-safe job_id parameter");
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

async fn declared_services_matching(name: &str) -> Result<Vec<service::ManagedService>, String> {
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
    service_failure(http_status("400"), "INVALID_REQUEST", message, false)
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

fn object_list_from_query(query: &str) -> Result<(String, String), Response> {
    let values = parse_qs(query);
    let raw_namespace = query_value(&values, "namespace").unwrap_or_default();
    if raw_namespace.is_empty() {
        return Err(send_json(
            http_status("400"),
            &json!({"error": "namespace is required"}),
        ));
    }
    let sentinel = crate::object_store::ObjectRef::new(&raw_namespace, "sentinel")
        .map_err(|error| send_json(http_status("400"), &json!({"error": error.to_string()})))?;
    let namespace = sentinel.namespace().to_string();
    let prefix = query_value(&values, "prefix")
        .unwrap_or_default()
        .trim_matches('/')
        .to_string();
    crate::object_store::ObjectRef::namespace_prefix(&namespace, &prefix)
        .map_err(|error| send_json(http_status("400"), &json!({"error": error.to_string()})))?;
    Ok((namespace, prefix))
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
// Host-header DNS-rebinding guard
// ---------------------------------------------------------------------------

/// Accept loopback Host values for direct local access. A configured HTTPS
/// reverse proxy may forward either DNS or IP Host values; because the listener
/// itself is loopback-only, `X-Forwarded-Proto` cannot be supplied by external
/// plaintext ingress. Dashboard authorization remains a separate boundary.
fn trusted_request_host(
    value: Option<&str>,
    forwarded_proto: Option<&str>,
    reverse_proxy_enabled: bool,
) -> bool {
    let Some(value) = value else { return false };
    if value.is_empty() {
        return false;
    }
    // Malformed authorities always fail closed.
    let proxy_https = reverse_proxy_enabled && forwarded_proto == Some("https");

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

fn valid_beacon_host(host: &str) -> bool {
    let bytes = host.as_bytes();
    !bytes.is_empty()
        && bytes.first().is_some_and(u8::is_ascii_alphanumeric)
        && bytes.last().is_some_and(u8::is_ascii_alphanumeric)
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || *byte == b'-')
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

/// Accept a bearer only after the request has resolved to one canonical
/// namespace and key boundary. Out-of-scope requests and bearer mismatches are
/// unauthorized; invalid configuration or an unavailable exact item is
/// reported separately so the route can return a redacted 503.
fn release_object_namespace(namespace: &str) -> bool {
    matches!(namespace, "releases" | "sources")
}

/// Route-scoped host beacon publication: the bearer stored as
/// `stado-host-health-api/token` and nothing else. Machine clients are
/// authorized separately through exact client policies; there is no global
/// dashboard bearer.
async fn authorize_host_health(request: &Request) -> bool {
    let expected = match crate::skarbiec::read_string("stado-host-health-api", "token").await {
        Ok(Some(value)) => value,
        Ok(None) | Err(_) => String::new(),
    };
    let authorization = request.header("authorization").unwrap_or("").trim();
    let supplied = authorization.strip_prefix("Bearer ").unwrap_or_default();
    !expected.is_empty() && constant_time_eq(expected.as_bytes(), supplied.as_bytes())
}

async fn authorize_object(
    request: &Request,
    namespace: &str,
    key_or_prefix: &str,
    list: bool,
    action: &str,
) -> Result<bool, ()> {
    let namespaces = config::object_api_namespaces().map_err(|_| ())?;
    let Some(policy) = namespaces.get(namespace) else {
        return Ok(false);
    };
    let in_scope = if list {
        policy
            .authorized_list_prefix(key_or_prefix, action)
            .is_some()
    } else {
        policy.allows_object_action(key_or_prefix, action)
    };
    if !in_scope {
        return Ok(false);
    }
    let expected = match crate::skarbiec::read_object_token(policy.item(), "token").await {
        Ok(Some(value)) if !value.is_empty() => value,
        Ok(_) => {
            eprintln!("[dashboard] object verifier item unavailable for namespace {namespace}");
            return Err(());
        }
        Err(error) => {
            eprintln!("[dashboard] object verifier failed for namespace {namespace}: {error}");
            return Err(());
        }
    };
    let authorization = request.header("authorization").unwrap_or("").trim();
    let supplied = authorization.strip_prefix("Bearer ").unwrap_or_default();
    Ok(constant_time_eq(expected.as_bytes(), supplied.as_bytes()))
}

/// Authenticate one immutable release publisher after resolving the exact
/// product prefix inside `stado://releases`. The former global object token is
/// never consulted.
async fn authorize_release(request: &Request, key_or_prefix: &str, list: bool) -> Result<bool, ()> {
    config::release_api_publishers().map_err(|_| ())?;
    let policy = if list {
        config::release_publisher_for_list(key_or_prefix).map(|(policy, _)| policy)
    } else {
        config::release_publisher_for_key(key_or_prefix)
    };
    let Some(policy) = policy else {
        return Ok(false);
    };
    let expected = crate::skarbiec::read_release_token(policy.item(), "token")
        .await
        .map_err(|_| ())?
        .filter(|value| !value.is_empty())
        .ok_or(())?;
    let authorization = request.header("authorization").unwrap_or("").trim();
    let supplied = authorization.strip_prefix("Bearer ").unwrap_or_default();
    Ok(constant_time_eq(expected.as_bytes(), supplied.as_bytes()))
}

async fn authorize_service(request: &Request, service: &str, action: &str) -> Result<bool, ()> {
    config::service_api_deployers().map_err(|_| ())?;
    let Some(policy) = config::service_deployer_for(service, action) else {
        return Ok(false);
    };
    let expected = crate::skarbiec::read_service_token(policy.item(), "token")
        .await
        .map_err(|_| ())?
        .filter(|value| !value.is_empty())
        .ok_or(())?;
    let authorization = request.header("authorization").unwrap_or("").trim();
    let supplied = authorization.strip_prefix("Bearer ").unwrap_or_default();
    Ok(constant_time_eq(expected.as_bytes(), supplied.as_bytes()))
}

fn machine_result_target(value: &Value) -> Option<&str> {
    value
        .get("job")
        .and_then(Value::as_object)
        .and_then(|job| job.get("provider"))
        .and_then(Value::as_str)
        .filter(|target| !target.is_empty())
}

async fn authenticate_machine_client(
    request: &Request,
    action: &str,
) -> Result<Option<&'static config::MachineApiClient>, ()> {
    let Some(supplied) = request
        .header("authorization")
        .and_then(|value| value.trim().strip_prefix("Bearer "))
        .filter(|value| !value.is_empty())
    else {
        return Ok(None);
    };
    let clients = config::machine_api_clients().map_err(|_| ())?;
    let mut matched = None;
    for client in clients
        .values()
        .filter(|client| client.allows_action(action))
    {
        let expected = crate::skarbiec::read_machine_token(client.item(), "token")
            .await
            .map_err(|_| ())?
            .filter(|value| !value.is_empty())
            .ok_or(())?;
        if constant_time_eq(expected.as_bytes(), supplied.as_bytes()) {
            if matched.is_some() {
                return Ok(None);
            }
            matched = Some(client);
        }
    }
    Ok(matched)
}
