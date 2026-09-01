//! `stado host weles-capture` and `stado host weles-capture-status`: put one
//! batch of `generic_capture` actions on a Weles worker host, then read what
//! the batch produced.
//!
//! The gap this closes: Weles has had the capture primitives all along and
//! exposed none of them, and the only ways into a worker host from here were
//! [`host_exec`](super::host_exec)'s read-only allowlist and a shell nobody is
//! allowed to open. So a plan for 1540 landing-page captures sat in
//! `product-guidelines` with `blockedBy: "no capture action exists on the Weles
//! worker"` written into it, and rendering happened on whichever laptop
//! somebody was sitting at — which is exactly the browser use the workspace
//! forbids off the dedicated host.
//!
//! Three properties this module keeps, because each one was a way the work
//! could have gone wrong:
//!
//! - **The plan is refused before the host is touched.** Every capture is
//!   checked against the contract — schema string, batch id, target, axis,
//!   step vocabulary, artifact prefix — and one bad entry refuses the whole
//!   plan by index. A partially enqueued batch is worse than a rejected one,
//!   because the half that landed still produces artifacts nobody planned.
//! - **The channel is held, not left behind.** `host forward-local` and
//!   `host forward-remote` exist to leave a forward up and write a marker for
//!   it; this one borrows the same option set through
//!   [`host_channel::ssh_options`] and holds the ssh process for the length of
//!   one command, so nothing survives the call on either side. The remote port
//!   comes from the service directory, never from an operator argument.
//! - **Status needs no memory of the enqueue.** Each action carries its own
//!   `artifact_prefix` in its params, so the state report is assembled from the
//!   worker's action log plus one storage listing. It answers the same on any
//!   control-plane host, including one that never ran the enqueue.

use std::net::TcpListener;
use std::time::{Duration, Instant};

use serde_json::{json, Map, Value};

use super::{host_channel, DeployError};
use crate::targets::ComputeTarget;

/// The plan document's schema string.
///
/// Checked rather than assumed: the field exists so that a JSON document
/// written for something else cannot be enqueued as 1540 browser sessions by
/// accident.
pub const PLAN_SCHEMA: &str = "wisent.weles-capture-plan.v1";

/// The one action this command enqueues, exactly as Weles registers it in its
/// dispatch table and in `weles-action-allowlist.txt`. The admission API
/// refuses any name outside that file, so there is nothing to select here.
pub const CAPTURE_ACTION: &str = "generic_capture";

/// Service-directory key retained for callers; its endpoint is the current
/// synchronous Weles API, not the removed database admission server.
const ADMISSION_SERVICE: &str = "weles-admission";

/// Namespace every capture artifact and sidecar lands in.
pub const ARTIFACT_NAMESPACE: &str = "weles-captures";

/// Credential item carrying the Weles Echo admission API bearer token, for a
/// host whose listener is configured to want one.
///
/// Read through Stado's selected credential store on THIS machine and sent as
/// an `Authorization` header. It is never written to a remote command line: a
/// token in `argv` on the far side is readable by every process on that host.
const ADMISSION_TOKEN_ITEM: &str = "echo-weles-api";
const ADMISSION_TOKEN_FIELD: &str = "token";

const RUN_ROUTE: &str = "/run";
const QUERY_ROUTE: &str = "/v1/echo/action-logs/query";
const QUERY_LIMIT: usize = 5000;
const MAX_STEPS: usize = 100;

/// The Weles API executes captures synchronously. The removed database-backed
/// admission API used to cap batches; there is no queue or chunk boundary now.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15 * 60);

/// How long the forward gets to start accepting connections. The ssh connect
/// half is already bounded by the inherited `ConnectTimeout`, so this bounds
/// only the local bind and the remote channel setup.
const FORWARD_DEADLINE: Duration = Duration::from_secs(20);

/// Gap between probes of the forwarded port.
const FORWARD_POLL: Duration = Duration::from_millis(100);

/// The five axes a landing-page capture belongs to. A sixth would be a change
/// to the capture contract, so an unknown one is refused instead of forwarded
/// to a worker that has no route for it.
pub const AXES: [&str; 5] = [
    "composition",
    "interaction",
    "reactivity",
    "state-change",
    "subpage",
];

/// The step vocabulary a capture may ask the worker for.
const STEP_OPS: [&str; 8] = [
    "wait_selector",
    "click",
    "hover",
    "focus",
    "press",
    "scroll",
    "wait_ms",
    "goto",
];

/// Exactly the keys a capture carries. An unexpected key is refused rather
/// than dropped: a plan writer who misspells `full_page` would otherwise get a
/// silently different capture and no way to tell from the report.
const CAPTURE_KEYS: [&str; 9] = [
    "batch",
    "site_slug",
    "source_url",
    "axis",
    "viewport",
    "full_page",
    "steps",
    "record_seconds",
    "artifact_prefix",
];

// ---------------------------------------------------------------------------
// The plan
// ---------------------------------------------------------------------------

/// One capture, validated. `params` is the object that goes to the worker
/// verbatim; the named fields are the ones this side reports and matches on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Capture {
    pub site_slug: String,
    pub axis: String,
    pub source_url: String,
    pub artifact_prefix: String,
    pub params: Map<String, Value>,
}

/// A `wisent.weles-capture-plan.v1` document, validated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    pub batch: String,
    pub target: String,
    pub captures: Vec<Capture>,
}

/// Read and validate a capture plan.
///
/// `target` is the host named on the command line: the plan states which host
/// it was written for, and a mismatch is refused rather than reconciled. A
/// plan whose artifact prefixes address one host's batch is not a plan for a
/// different host.
pub fn parse_plan(path: &str, target: &str, batch: Option<&str>) -> Result<Plan, DeployError> {
    let bytes = std::fs::read(path)
        .map_err(|error| DeployError(format!("capture plan {path} cannot be read: {error}")))?;
    let document: Value = serde_json::from_slice(&bytes).map_err(|error| {
        DeployError(format!("capture plan {path} is not readable JSON: {error}"))
    })?;
    let document = document
        .as_object()
        .ok_or_else(|| DeployError(format!("capture plan {path} must be a JSON object")))?;
    if document.get("schema").and_then(Value::as_str) != Some(PLAN_SCHEMA) {
        return Err(DeployError(format!(
            "capture plan must declare the schema {PLAN_SCHEMA}"
        )));
    }

    let batch = batch
        .or_else(|| document.get("batch").and_then(Value::as_str))
        .unwrap_or_default()
        .trim()
        .to_string();
    if batch.is_empty() {
        return Err(DeployError(
            "capture plan must name a batch id, or --batch must supply one".to_string(),
        ));
    }
    safe_component("capture batch id", &batch)?;

    let declared = document
        .get("target")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim();
    if declared.is_empty() {
        return Err(DeployError(
            "capture plan must name the target host it was written for".to_string(),
        ));
    }
    if declared != target {
        return Err(DeployError(format!(
            "capture plan was written for target {declared}, not {target}"
        )));
    }

    let entries = document
        .get("captures")
        .and_then(Value::as_array)
        .ok_or_else(|| DeployError("capture plan must carry a captures array".to_string()))?;
    if entries.is_empty() {
        return Err(DeployError(
            "capture plan carries no captures, so there is nothing to enqueue".to_string(),
        ));
    }
    let mut captures = Vec::with_capacity(entries.len());
    for (index, entry) in entries.iter().enumerate() {
        captures.push(parse_capture(index + 1, entry, &batch)?);
    }
    Ok(Plan {
        batch,
        target: declared.to_string(),
        captures,
    })
}

/// One capture entry, checked field by field. `index` is one-based because it
/// appears in every refusal and an operator counts a list from one.
fn parse_capture(index: usize, entry: &Value, batch: &str) -> Result<Capture, DeployError> {
    let object = entry
        .as_object()
        .ok_or_else(|| DeployError(format!("capture {index} is not a JSON object")))?;
    for key in object.keys() {
        if !CAPTURE_KEYS.contains(&key.as_str()) {
            return Err(DeployError(format!(
                "capture {index} carries the key {key}, which is not one of {}",
                CAPTURE_KEYS.join(", ")
            )));
        }
    }

    let site_slug = text_field(object, "site_slug");
    if site_slug.is_empty() {
        return Err(DeployError(format!(
            "capture {index} must carry a non-empty site_slug"
        )));
    }
    let source_url = text_field(object, "source_url");
    if !source_url.starts_with("http://") && !source_url.starts_with("https://") {
        return Err(DeployError(format!(
            "capture {index} source_url must be an http or https URL"
        )));
    }
    let axis = text_field(object, "axis");
    if !AXES.contains(&axis.as_str()) {
        return Err(DeployError(format!(
            "capture {index} axis must be one of {}",
            AXES.join(", ")
        )));
    }

    let viewport = object.get("viewport").and_then(Value::as_object);
    let positive = |key: &str| {
        viewport
            .and_then(|viewport| viewport.get(key))
            .and_then(Value::as_f64)
            .is_some_and(|value| value > f64::default())
    };
    if !positive("width") || !positive("height") || !positive("device_scale_factor") {
        return Err(DeployError(format!(
            "capture {index} viewport must carry a positive width, height and device_scale_factor"
        )));
    }
    if object.get("full_page").and_then(Value::as_bool).is_none() {
        return Err(DeployError(format!(
            "capture {index} full_page must be true or false"
        )));
    }
    if !object
        .get("record_seconds")
        .and_then(Value::as_f64)
        .is_some_and(|seconds| seconds >= f64::default())
    {
        return Err(DeployError(format!(
            "capture {index} record_seconds must be a number of seconds that is zero or more"
        )));
    }

    let steps = object
        .get("steps")
        .and_then(Value::as_array)
        .ok_or_else(|| {
            DeployError(format!(
                "capture {index} steps must be an array of objects carrying op and value"
            ))
        })?;
    if steps.len() > MAX_STEPS {
        return Err(DeployError(format!(
            "capture {index} carries {} steps; Weles accepts at most {MAX_STEPS} per capture",
            steps.len()
        )));
    }
    for (position, step) in steps.iter().enumerate() {
        let op = step
            .as_object()
            .and_then(|step| step.get("op"))
            .and_then(Value::as_str)
            .unwrap_or_default();
        if !STEP_OPS.contains(&op) {
            return Err(DeployError(format!(
                "capture {index} step {} names the operation {op}, which is not one of {}",
                position + 1,
                STEP_OPS.join(", ")
            )));
        }
    }

    let artifact_prefix = text_field(object, "artifact_prefix");
    let batch_root = format!("stado://{ARTIFACT_NAMESPACE}/{batch}/");
    if !artifact_prefix.starts_with(&batch_root) {
        return Err(DeployError(format!(
            "capture {index} artifact_prefix must be under {batch_root}"
        )));
    }
    if !artifact_prefix.ends_with('/') {
        return Err(DeployError(format!(
            "capture {index} artifact_prefix must end with '/'"
        )));
    }
    let key = artifact_prefix.trim_start_matches("stado://").to_string();
    if key
        .trim_end_matches('/')
        .split('/')
        .any(|part| part.is_empty() || part == "." || part == "..")
        || key.contains('\\')
    {
        return Err(DeployError(format!(
            "capture {index} artifact_prefix must not contain an empty, '.' or '..' path segment"
        )));
    }

    // The params object reaches the worker as written, with `batch` set to the
    // batch this run resolved: `--batch` has to reach the artifact sidecars,
    // not just the enqueue report.
    let mut params = object.clone();
    params.insert("batch".to_string(), Value::from(batch));
    Ok(Capture {
        site_slug,
        axis,
        source_url,
        artifact_prefix,
        params,
    })
}

fn text_field(object: &Map<String, Value>, key: &str) -> String {
    object
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_string()
}

/// The character set a batch id may use, identical to the one the forward and
/// release commands accept for a name. The id becomes a storage key segment
/// and a query filter, and both of those are the reason not to widen it.
fn safe_component(kind: &str, value: &str) -> Result<(), DeployError> {
    if value.is_empty()
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
    {
        return Err(DeployError(format!(
            "{kind} must contain only letters, digits, '.', '_' or '-'"
        )));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// The admission API
// ---------------------------------------------------------------------------

/// Where one host's Weles admission API listens, as the service directory
/// declares it.
#[derive(Debug, Clone)]
pub struct Admission {
    pub target: ComputeTarget,
    /// The loopback port the API binds ON THE TARGET.
    pub port: u16,
    /// The directory's own address string, for the report.
    pub declared_url: String,
}

/// Resolve TARGET and the admission endpoint it serves.
///
/// Both come from the canonical registry: [`host_channel::resolve_target`]
/// makes the same refusals every other host command makes, and the service
/// directory supplies the port. `active_host` is checked because these
/// endpoints are keyed by the machine ASKING — the entry for the serving host
/// is the loopback address it serves on, and no other host's entry is.
pub async fn resolve_admission(target: &str) -> Result<Admission, DeployError> {
    let registry = host_channel::canonical_registry().await?;
    let resolved = host_channel::resolve_target(&registry, target)?.clone();
    let service = registry.service(ADMISSION_SERVICE).ok_or_else(|| {
        DeployError(format!(
            "the service directory carries no {ADMISSION_SERVICE} entry, so nothing declares where the Weles admission API listens"
        ))
    })?;
    if service.active_host != resolved.name {
        return Err(DeployError(format!(
            "the service directory says {ADMISSION_SERVICE} runs on {}, not on {}",
            service.active_host, resolved.name
        )));
    }
    let endpoint = service.address_for(&resolved.name).ok_or_else(|| {
        DeployError(format!(
            "the service directory declares no {ADMISSION_SERVICE} address for {}",
            resolved.name
        ))
    })?;
    let url = url::Url::parse(&endpoint.url).map_err(|error| {
        DeployError(format!(
            "the {ADMISSION_SERVICE} address {} is not a URL: {error}",
            endpoint.url
        ))
    })?;
    let loopback = matches!(url.host_str(), Some("127.0.0.1" | "::1" | "localhost"));
    let port = url.port_or_known_default();
    match (url.scheme(), loopback, port) {
        ("http", true, Some(port)) => Ok(Admission {
            target: resolved,
            port,
            declared_url: endpoint.url.clone(),
        }),
        _ => Err(DeployError(format!(
            "the {ADMISSION_SERVICE} address {} is not a loopback http listener, and this command forwards a loopback port rather than dialling anything else",
            endpoint.url
        ))),
    }
}

/// What Stado's credential store had to say about the admission bearer token.
#[derive(Debug, Clone)]
enum Token {
    /// The store holds one, and every request carries it.
    Present(String),
    /// The store answered and holds none — the documented state for a
    /// listener bound to loopback, which serves unauthenticated.
    Absent,
    /// The store did not answer. Not fatal by itself, because a loopback
    /// listener may want no token at all; the API's own 401 is what decides,
    /// and this string is what that refusal then reports.
    Unreadable(String),
}

impl Token {
    fn describe(&self) -> String {
        match self {
            Self::Present(_) => format!(
                "the token stored as {ADMISSION_TOKEN_ITEM}.{ADMISSION_TOKEN_FIELD} was rejected"
            ),
            Self::Absent => format!(
                "Stado's credential store holds no {ADMISSION_TOKEN_ITEM}.{ADMISSION_TOKEN_FIELD}"
            ),
            Self::Unreadable(error) => format!(
                "Stado's credential store could not be read for {ADMISSION_TOKEN_ITEM}.{ADMISSION_TOKEN_FIELD}: {error}"
            ),
        }
    }

    fn state(&self) -> &'static str {
        match self {
            Self::Present(_) => "present",
            Self::Absent => "absent",
            Self::Unreadable(_) => "unreadable",
        }
    }
}

async fn read_token() -> Token {
    match crate::credential_store::read_string(ADMISSION_TOKEN_ITEM, ADMISSION_TOKEN_FIELD).await {
        Ok(Some(token)) if !token.trim().is_empty() => Token::Present(token.trim().to_string()),
        Ok(_) => Token::Absent,
        Err(error) => Token::Unreadable(error.to_string()),
    }
}

/// An open path to one host's loopback admission API, alive for exactly as
/// long as this value is.
pub struct Channel {
    /// The ssh process holding the forward, or `None` when the target is this
    /// machine and there is no hop to make. `kill_on_drop` is what makes the
    /// forward end with the command: nothing is daemonised and no marker is
    /// written, which is the whole difference from `host forward-remote`.
    forward: Option<tokio::process::Child>,
    base_url: String,
    client: reqwest::Client,
    token: Token,
}

/// Open the channel to a resolved admission endpoint.
pub async fn open_channel(admission: &Admission) -> Result<Channel, DeployError> {
    let token = read_token().await;
    let client = reqwest::Client::builder()
        .timeout(REQUEST_TIMEOUT)
        .build()
        .map_err(|error| {
            DeployError(format!(
                "cannot build the Weles admission API client: {error}"
            ))
        })?;
    if host_channel::target_is_this_host(&admission.target) {
        return Ok(Channel {
            forward: None,
            base_url: format!("http://127.0.0.1:{}", admission.port),
            client,
            token,
        });
    }
    let ssh = admission
        .target
        .ssh
        .as_deref()
        .ok_or_else(|| DeployError("registry target has no SSH destination".to_string()))?;
    let local_port = free_loopback_port()?;
    let mut argv = host_channel::ssh_options(ssh);
    let destination = argv
        .pop()
        .ok_or_else(|| DeployError("SSH channel has no destination".to_string()))?;
    argv.extend([
        "-N".to_string(),
        "-o".to_string(),
        "ExitOnForwardFailure=yes".to_string(),
        "-o".to_string(),
        "ServerAliveInterval=30".to_string(),
        "-o".to_string(),
        "ServerAliveCountMax=3".to_string(),
        "-L".to_string(),
        format!("127.0.0.1:{local_port}:127.0.0.1:{}", admission.port),
        destination,
    ]);
    let (program, arguments) = argv
        .split_first()
        .ok_or_else(|| DeployError("SSH channel is empty".to_string()))?;
    let mut child = tokio::process::Command::new(program)
        .args(arguments)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|error| {
            DeployError(format!(
                "cannot start SSH forwarding to the Weles admission API: {error}"
            ))
        })?;
    await_forward(&mut child, local_port).await?;
    Ok(Channel {
        forward: Some(child),
        base_url: format!("http://127.0.0.1:{local_port}"),
        client,
        token,
    })
}

/// A loopback port the kernel says is free right now.
///
/// A fixed port would collide with the operator's own `host forward-remote`
/// marker and with a second capture run beside this one.
/// `ExitOnForwardFailure=yes` turns the residual race — the port taken between
/// this probe and ssh's bind — into an immediate refusal instead of a silent
/// misroute to whatever took it.
fn free_loopback_port() -> Result<u16, DeployError> {
    let listener = TcpListener::bind(("127.0.0.1", u16::default())).map_err(|error| {
        DeployError(format!(
            "cannot reserve a loopback port for the admission forward: {error}"
        ))
    })?;
    let port = listener
        .local_addr()
        .map_err(|error| DeployError(format!("cannot read the reserved loopback port: {error}")))?
        .port();
    Ok(port)
}

/// Wait until the forwarded port accepts a connection, or until ssh gives up
/// and says why. A forward that is reported open before anything is listening
/// is how a connection refused ends up looking like a dead API.
async fn await_forward(child: &mut tokio::process::Child, port: u16) -> Result<(), DeployError> {
    let deadline = Instant::now() + FORWARD_DEADLINE;
    loop {
        if let Some(status) = child
            .try_wait()
            .map_err(|error| DeployError(format!("cannot read the SSH forward's state: {error}")))?
        {
            return Err(DeployError(format!(
                "SSH forwarding to the Weles admission API exited ({status}): {}",
                forward_error(child).await
            )));
        }
        if tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .is_ok()
        {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(DeployError(format!(
                "SSH forwarding to the Weles admission API did not accept a connection on 127.0.0.1:{port} within {} seconds",
                FORWARD_DEADLINE.as_secs()
            )));
        }
        tokio::time::sleep(FORWARD_POLL).await;
    }
}

/// ssh's own last word, verbatim — a refused key, a rejected bind, a host that
/// is not answering. A paraphrase here would cost the operator the one line
/// that names the cause.
async fn forward_error(child: &mut tokio::process::Child) -> String {
    let Some(mut stderr) = child.stderr.take() else {
        return "ssh forwarding failed".to_string();
    };
    let mut detail = String::new();
    use tokio::io::AsyncReadExt as _;
    if stderr.read_to_string(&mut detail).await.is_err() {
        return "ssh forwarding failed".to_string();
    }
    detail
        .lines()
        .rfind(|line| !line.trim().is_empty())
        .unwrap_or("ssh forwarding failed")
        .to_string()
}

impl Channel {
    /// The state of the bearer token this channel is using, for the report.
    pub fn token_state(&self) -> &'static str {
        self.token.state()
    }

    /// Whether the call crossed an ssh forward or stayed on this machine's
    /// loopback. The two answers are different claims about what was reached,
    /// and a report that says only `127.0.0.1` cannot tell them apart — which
    /// is how a marker naming a port nothing had ever bound went unnoticed on
    /// this fleet for weeks.
    pub fn transport(&self) -> &'static str {
        if self.forward.is_some() {
            "ssh"
        } else {
            "loopback"
        }
    }

    async fn json_request(
        &self,
        method: reqwest::Method,
        route: &str,
        body: Option<&Value>,
    ) -> Result<(reqwest::StatusCode, String, Value), DeployError> {
        let mut request = self
            .client
            .request(method, format!("{}{route}", self.base_url));
        if let Some(body) = body {
            request = request.json(body);
        }
        if let Token::Present(token) = &self.token {
            request = request.bearer_auth(token);
        }
        let response = request.send().await.map_err(|error| {
            DeployError(format!("the Weles API did not answer {route}: {error}"))
        })?;
        let status = response.status();
        let body = response.text().await.map_err(|error| {
            DeployError(format!(
                "the Weles API answered {route} unreadably: {error}"
            ))
        })?;
        let payload: Value = serde_json::from_str(&body).unwrap_or(Value::Null);
        if status == reqwest::StatusCode::UNAUTHORIZED {
            return Err(DeployError(format!(
                "the Weles API requires a bearer token and {}",
                self.token.describe()
            )));
        }
        Ok((status, body, payload))
    }

    fn require_success(
        route: &str,
        status: reqwest::StatusCode,
        body: &str,
        payload: Value,
    ) -> Result<Value, DeployError> {
        if status.is_success() && payload.get("ok").and_then(Value::as_bool) == Some(true) {
            return Ok(payload);
        }
        let detail = payload
            .get("error")
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or_else(|| {
                body.lines()
                    .rfind(|line| !line.trim().is_empty())
                    .unwrap_or("no reason given")
                    .to_string()
            });
        Err(DeployError(format!(
            "the Weles API refused {route} with {status}: {detail}"
        )))
    }

    /// One Weles API call. Successful responses are returned in full because
    /// the synchronous `/run` surface has no `{ data }` envelope.
    async fn call(&self, route: &str, body: &Value) -> Result<Value, DeployError> {
        let (status, response_body, payload) = self
            .json_request(reqwest::Method::POST, route, Some(body))
            .await?;
        Self::require_success(route, status, &response_body, payload)
    }

    /// Keep a completed `/run` envelope even when its trajectory failed. Weles
    /// writes browser diagnostics before returning that 502; treating the HTTP
    /// status as if no run happened discards the exact network record needed to
    /// explain the failure.
    async fn observe_run(&self, body: &Value) -> Result<Value, DeployError> {
        let (status, response_body, payload) = self
            .json_request(reqwest::Method::POST, RUN_ROUTE, Some(body))
            .await?;
        if payload
            .get("run_id")
            .and_then(Value::as_str)
            .is_some_and(|run_id| !run_id.is_empty())
        {
            return Ok(payload);
        }
        Self::require_success(RUN_ROUTE, status, &response_body, payload)
    }

    async fn get_json(&self, route: &str) -> Result<Value, DeployError> {
        let (status, response_body, payload) =
            self.json_request(reqwest::Method::GET, route, None).await?;
        Self::require_success(route, status, &response_body, payload)
    }

    async fn get_bytes(&self, route: &str) -> Result<Vec<u8>, DeployError> {
        let mut request = self.client.get(format!("{}{route}", self.base_url));
        if let Token::Present(token) = &self.token {
            request = request.bearer_auth(token);
        }
        let response = request.send().await.map_err(|error| {
            DeployError(format!("the Weles API did not answer {route}: {error}"))
        })?;
        let status = response.status();
        if status == reqwest::StatusCode::UNAUTHORIZED {
            return Err(DeployError(format!(
                "the Weles API requires a bearer token and {}",
                self.token.describe()
            )));
        }
        if !status.is_success() {
            let detail = response
                .text()
                .await
                .unwrap_or_else(|_| "no reason given".to_string());
            return Err(DeployError(format!(
                "the Weles API refused {route} with {status}: {detail}"
            )));
        }
        response
            .bytes()
            .await
            .map(|bytes| bytes.to_vec())
            .map_err(|error| {
                DeployError(format!(
                    "the Weles API answered {route} unreadably: {error}"
                ))
            })
    }
}

/// Run one fixed non-capture action through Weles's synchronous API and retain
/// its complete redacted result.
pub async fn run_action_payload(
    channel: &Channel,
    action: &str,
    params: Value,
) -> Result<Value, DeployError> {
    channel
        .call(
            RUN_ROUTE,
            &json!({
                "action": action,
                "params": params,
                "creds": "redact",
                "timeout_ms": REQUEST_TIMEOUT.as_millis(),
            }),
        )
        .await
}

/// Run one fixed browser action and keep its run envelope even when the
/// trajectory itself fails after navigation. The envelope carries the run id
/// needed to read Weles's completed browser diagnostics.
pub async fn observe_action_payload(
    channel: &Channel,
    action: &str,
    params: Value,
    account_id: Option<&str>,
    fresh_profile: bool,
) -> Result<Value, DeployError> {
    channel
        .observe_run(&run_request(action, params, account_id, fresh_profile))
        .await
}

/// The `/run` body, built where it can be read in a test.
///
/// `account_id` is the account binding the Weles API turns into `ACCOUNT_ID`
/// in the trajectory's environment, which is what keys the browser profile
/// directory. Absent it, every run is a new device to the site being driven -
/// which is how five sign-ins in a row each met a first-visit risk check, one
/// of them a passkey demand.
pub fn run_request(
    action: &str,
    params: Value,
    account_id: Option<&str>,
    fresh_profile: bool,
) -> Value {
    let mut request = json!({
        "action": action,
        "params": params,
        "creds": "redact",
        "timeout_ms": REQUEST_TIMEOUT.as_millis(),
    });
    if let Some(account_id) = account_id {
        request["account_id"] = json!(account_id);
    }
    if fresh_profile {
        request["fresh_profile"] = json!(true);
    }
    request
}

/// One account id, refused unless it is safe to use as an identity and a
/// directory key.
///
/// Weles hashes it for the profile directory and the API echoes it into the
/// child environment, so anything outside this alphabet is refused here rather
/// than resolved on the host.
pub fn checked_account_id(account_id: &str) -> Result<&str, DeployError> {
    if account_id.is_empty()
        || account_id.len() > 128
        || !account_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
    {
        return Err(DeployError(format!(
            "account id {account_id:?} must be 1-128 characters of letters, digits, '-', '_' or '.'"
        )));
    }
    Ok(account_id)
}

fn diagnostic_run_id(run_id: &str) -> Result<&str, DeployError> {
    if run_id.is_empty()
        || run_id.len() > 128
        || !run_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err(DeployError(
            "Weles diagnostic run id contains an unsafe character".to_string(),
        ));
    }
    Ok(run_id)
}

fn public_resource_url(raw: &str) -> String {
    match url::Url::parse(raw) {
        Ok(mut parsed) => {
            parsed.set_query(None);
            parsed.set_fragment(None);
            parsed.to_string()
        }
        Err(_) => raw.split(['?', '#']).next().unwrap_or_default().to_string(),
    }
}

fn resource_host(raw: &str) -> Option<String> {
    url::Url::parse(raw)
        .ok()
        .and_then(|parsed| parsed.host_str().map(str::to_string))
}

fn is_stado_object_url(raw: &str) -> bool {
    url::Url::parse(raw).is_ok_and(|parsed| parsed.path() == "/api/stado/object")
}

fn is_legacy_cloud_image_url(raw: &str) -> bool {
    let Some(host) = resource_host(raw) else {
        return false;
    };
    [
        "amazonaws.com",
        "blob.core.windows.net",
        "cloudfront.net",
        "googleapis.com",
        "storage.cloud.google.com",
    ]
    .iter()
    .any(|suffix| {
        host == *suffix
            || host
                .strip_suffix(suffix)
                .is_some_and(|prefix| prefix.ends_with('.'))
    })
}

fn response_content_type(event: &Value) -> &str {
    event
        .get("headers")
        .and_then(Value::as_object)
        .and_then(|headers| {
            headers
                .get("content-type")
                .or_else(|| headers.get("Content-Type"))
        })
        .and_then(Value::as_str)
        .unwrap_or_default()
}

/// Summarize only image request metadata from one completed Weles browser run.
///
/// Signed query strings and response bodies stay on the worker. The report
/// exposes host/path, status and counts only.
pub async fn image_diagnostics(channel: &Channel, run_id: &str) -> Result<Value, DeployError> {
    const MAX_DIAGNOSTIC_BYTES: u64 = 128 * 1024 * 1024;

    let run_id = diagnostic_run_id(run_id)?;
    let manifest_route = format!("/diagnostics/{run_id}");
    let manifest = channel.get_json(&manifest_route).await?;
    let files = manifest
        .get("files")
        .and_then(Value::as_array)
        .ok_or_else(|| DeployError("Weles diagnostics returned no file inventory".to_string()))?;
    let recording = files
        .iter()
        .filter(|file| {
            file.get("path")
                .and_then(Value::as_str)
                .is_some_and(|path| path.ends_with(".inst.json"))
        })
        .max_by_key(|file| {
            file.get("bytes")
                .and_then(Value::as_u64)
                .unwrap_or_default()
        })
        .ok_or_else(|| {
            DeployError("Weles diagnostics contain no browser network recording".to_string())
        })?;
    let recording_path = recording
        .get("path")
        .and_then(Value::as_str)
        .ok_or_else(|| DeployError("Weles diagnostic recording has no path".to_string()))?;
    let expected_bytes = recording
        .get("bytes")
        .and_then(Value::as_u64)
        .ok_or_else(|| DeployError("Weles diagnostic recording has no byte size".to_string()))?;
    if expected_bytes > MAX_DIAGNOSTIC_BYTES {
        return Err(DeployError(format!(
            "Weles browser network recording is {expected_bytes} bytes; the read-only image inspector accepts at most {MAX_DIAGNOSTIC_BYTES}"
        )));
    }
    let encoded_path =
        url::form_urlencoded::byte_serialize(recording_path.as_bytes()).collect::<String>();
    let recording_route = format!("/diagnostics/{run_id}/file?path={encoded_path}");
    let recording_bytes = channel.get_bytes(&recording_route).await?;
    if recording_bytes.len() as u64 != expected_bytes {
        return Err(DeployError(format!(
            "Weles diagnostic recording changed while it was read: manifest={expected_bytes} downloaded={}",
            recording_bytes.len()
        )));
    }
    let document: Value = serde_json::from_slice(&recording_bytes)
        .map_err(|error| DeployError(format!("Weles browser recording is not JSON: {error}")))?;
    let events = document
        .get("requests")
        .and_then(Value::as_array)
        .ok_or_else(|| DeployError("Weles browser recording has no request events".to_string()))?;

    let mut image_urls = std::collections::BTreeSet::new();
    for event in events {
        let url = event.get("url").and_then(Value::as_str).unwrap_or_default();
        if url.is_empty() {
            continue;
        }
        let requested_image = event.get("phase").and_then(Value::as_str) == Some("req")
            && event.get("resourceType").and_then(Value::as_str) == Some("image");
        let returned_image = event.get("phase").and_then(Value::as_str) == Some("res")
            && response_content_type(event)
                .to_ascii_lowercase()
                .starts_with("image/");
        if requested_image || returned_image || is_stado_object_url(url) {
            image_urls.insert(url.to_string());
        }
    }

    let mut response_statuses = std::collections::BTreeMap::new();
    let mut request_failures = std::collections::BTreeMap::new();
    for event in events {
        let url = event.get("url").and_then(Value::as_str).unwrap_or_default();
        if !image_urls.contains(url) {
            continue;
        }
        match event.get("phase").and_then(Value::as_str) {
            Some("res") => {
                if let Some(status) = event.get("status").and_then(Value::as_u64) {
                    response_statuses.insert(url.to_string(), status);
                }
            }
            Some("reqfailed") => {
                let reason = event
                    .get("failure")
                    .and_then(|failure| {
                        failure
                            .get("errorText")
                            .or_else(|| failure.get("error_text"))
                    })
                    .and_then(Value::as_str)
                    .unwrap_or("request failed");
                request_failures.insert(url.to_string(), reason.to_string());
            }
            _ => {}
        }
    }

    let mut hosts = std::collections::BTreeMap::<String, u64>::new();
    let mut statuses = std::collections::BTreeMap::<String, u64>::new();
    let mut stado_statuses = std::collections::BTreeMap::<String, u64>::new();
    let mut failed_resources = Vec::new();
    let mut loaded_images = 0_u64;
    let mut unresolved_images = 0_u64;
    let mut stado_object_images = 0_u64;
    let mut legacy_cloud_images = 0_u64;
    for url in &image_urls {
        if let Some(host) = resource_host(url) {
            *hosts.entry(host).or_default() += 1;
        }
        let stado_object = is_stado_object_url(url);
        if stado_object {
            stado_object_images += 1;
        }
        if is_legacy_cloud_image_url(url) {
            legacy_cloud_images += 1;
        }
        if let Some(reason) = request_failures.get(url) {
            failed_resources.push(json!({
                "url": public_resource_url(url),
                "failure": reason,
            }));
            continue;
        }
        match response_statuses.get(url).copied() {
            Some(status) => {
                *statuses.entry(status.to_string()).or_default() += 1;
                if stado_object {
                    *stado_statuses.entry(status.to_string()).or_default() += 1;
                }
                if (200..400).contains(&status) {
                    loaded_images += 1;
                } else {
                    failed_resources.push(json!({
                        "url": public_resource_url(url),
                        "status": status,
                    }));
                }
            }
            None => unresolved_images += 1,
        }
    }

    Ok(json!({
        "run_id": run_id,
        "recording_file": recording_path,
        "network_events": events.len(),
        "image_requests": image_urls.len(),
        "loaded_images": loaded_images,
        "failed_images": failed_resources.len(),
        "unresolved_images": unresolved_images,
        "stado_object_images": stado_object_images,
        "legacy_cloud_images": legacy_cloud_images,
        "image_hosts": hosts,
        "status_counts": statuses,
        "stado_status_counts": stado_statuses,
        "failed_resources": failed_resources,
        "page_errors": document
            .get("pageerrors")
            .and_then(Value::as_array)
            .map_or(0, Vec::len),
    }))
}

/// Run one non-capture action through Weles's synchronous API.
///
/// Callers still name a fixed action in product code; this function does not
/// expose an arbitrary-action CLI.
pub async fn run_action(
    channel: &Channel,
    action: &str,
    params: Value,
) -> Result<String, DeployError> {
    let data = run_action_payload(channel, action, params).await?;
    data.get("run_id")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
        .map(str::to_string)
        .ok_or_else(|| {
            DeployError("the Weles API completed the action and returned no run id".to_string())
        })
}

/// Read the newest recorded row for one fixed action.
pub async fn latest_action_log(
    channel: &Channel,
    action: &str,
) -> Result<Option<Value>, DeployError> {
    let data = match channel
        .call(
            QUERY_ROUTE,
            &json!({ "action": action, "limit": QUERY_LIMIT }),
        )
        .await
    {
        Ok(data) => data,
        Err(error)
            if error
                .0
                .contains("refused /v1/echo/action-logs/query with 404 Not Found") =>
        {
            return Ok(None);
        }
        Err(error) => return Err(error),
    };
    let logs = data
        .get("logs")
        .and_then(Value::as_array)
        .ok_or_else(|| DeployError("the Weles admission API returned no action log".to_string()))?;
    Ok(logs.first().cloned())
}

// ---------------------------------------------------------------------------
// Enqueue
// ---------------------------------------------------------------------------

/// One accepted capture and the action id the worker will run it under.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Enqueued {
    pub action_id: String,
    pub site_slug: String,
    pub axis: String,
    pub artifact_prefix: String,
}

/// Execute every capture through Weles's current synchronous `/run` surface.
///
/// The old admission API and its database queue were removed from Weles.
/// Returning only after each run finishes means an accepted row already has a
/// final run id and its artifacts have either been uploaded or the command has
/// failed with the worker's exact reason.
pub async fn enqueue(channel: &Channel, plan: &Plan) -> Result<Vec<Enqueued>, DeployError> {
    let mut accepted = Vec::with_capacity(plan.captures.len());
    for capture in &plan.captures {
        let payload = channel
            .call(
                RUN_ROUTE,
                &json!({
                    "action": CAPTURE_ACTION,
                    "params": Value::Object(capture.params.clone()),
                    "creds": "redact",
                    "timeout_ms": REQUEST_TIMEOUT.as_millis(),
                }),
            )
            .await?;
        let run_id = payload
            .get("run_id")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| {
                DeployError(
                    "the Weles API completed the capture and returned no run id".to_string(),
                )
            })?;
        accepted.push(Enqueued {
            action_id: run_id.to_string(),
            site_slug: capture.site_slug.clone(),
            axis: capture.axis.clone(),
            artifact_prefix: capture.artifact_prefix.clone(),
        });
    }
    Ok(accepted)
}

// ---------------------------------------------------------------------------
// Status
// ---------------------------------------------------------------------------

/// The four states this command reports.
pub const STATE_QUEUED: &str = "queued";
pub const STATE_RUNNING: &str = "running";
pub const STATE_DONE: &str = "done";
pub const STATE_FAILED: &str = "failed";

/// One enqueued capture as the worker's action log and the object store
/// describe it now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaptureState {
    pub action_id: String,
    pub site_slug: String,
    pub axis: String,
    pub state: String,
    pub error: Option<String>,
    pub artifact_prefix: String,
    /// Artifact and sidecar URIs already under this capture's prefix.
    pub artifacts: Vec<String>,
}

/// Translate the action log's own status word.
///
/// Weles writes `queued` on enqueue, `running` on claim and `completed` or
/// `failed` when it records the result. Only `completed` is renamed, and a
/// word this table does not know is passed through verbatim: folding an
/// unrecognised status into one of ours would be a verdict nobody measured.
fn capture_state(status: &str) -> String {
    if status == "completed" {
        return STATE_DONE.to_string();
    }
    status.to_string()
}

/// One batch as the worker's action log and the object store describe it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BatchStatus {
    pub captures: Vec<CaptureState>,
    /// Why the artifact listing could not be made, when it could not.
    ///
    /// `None` means the store answered and every `artifacts` list is what is
    /// really there. `Some` means nobody could ask, and the empty lists are the
    /// absence of an answer rather than the absence of objects — the same
    /// distinction `stado storage ls` draws between an empty prefix and an
    /// unreachable one, and for the same reason: on this fleet those two states
    /// were indistinguishable through one method's return value, and a
    /// forbidden store read exactly like a drained one.
    pub artifacts_unreachable: Option<String>,
}

/// Per-action state of one batch, plus the artifact keys already stored.
///
/// One action-log query and one storage listing. Rows are matched to the batch
/// by the `batch` param each action carries, and artifacts to the action by
/// the `artifact_prefix` it carries, so this reads correctly from a machine
/// that never ran the enqueue.
pub async fn status(channel: &Channel, batch: &str) -> Result<BatchStatus, DeployError> {
    safe_component("capture batch id", batch)?;
    let data = channel
        .call(
            QUERY_ROUTE,
            &json!({ "action": CAPTURE_ACTION, "limit": QUERY_LIMIT }),
        )
        .await?;
    let logs = data.get("logs").and_then(Value::as_array).ok_or_else(|| {
        DeployError(
            "the Weles admission API returned no action log for the capture action".to_string(),
        )
    })?;
    // A store that will not answer is not a batch with no artifacts. The
    // action states are still worth having, so the failure is carried beside
    // them instead of replacing them.
    let (artifacts, artifacts_unreachable) = match batch_artifacts(batch).await {
        Ok(artifacts) => (artifacts, None),
        Err(error) => (Vec::new(), Some(error.to_string())),
    };
    let mut states: Vec<CaptureState> = logs
        .iter()
        .filter(|row| row.pointer("/params/batch").and_then(Value::as_str) == Some(batch))
        .map(|row| {
            let artifact_prefix = row
                .pointer("/params/artifact_prefix")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            let owned = if artifact_prefix.is_empty() {
                Vec::new()
            } else {
                artifacts
                    .iter()
                    .filter(|uri| uri.starts_with(&artifact_prefix))
                    .cloned()
                    .collect()
            };
            CaptureState {
                action_id: row
                    .get("id")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                site_slug: row
                    .pointer("/params/site_slug")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                axis: row
                    .pointer("/params/axis")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                state: capture_state(
                    row.get("status")
                        .and_then(Value::as_str)
                        .unwrap_or_default(),
                ),
                error: row
                    .get("error")
                    .and_then(Value::as_str)
                    .map(str::to_string)
                    .filter(|error| !error.trim().is_empty()),
                artifact_prefix,
                artifacts: owned,
            }
        })
        .collect();
    // The action log is ordered by start time, which puts every capture that
    // has not started yet in one undifferentiated block. Site and axis are
    // what an operator scans for, so that is the order the report prints in.
    states.sort_by(|left, right| {
        (&left.site_slug, &left.axis, &left.action_id).cmp(&(
            &right.site_slug,
            &right.axis,
            &right.action_id,
        ))
    });
    Ok(BatchStatus {
        captures: states,
        artifacts_unreachable,
    })
}

/// Every capture object already in Stado storage under one batch, through the
/// same provider-neutral surface `stado storage objects` and `storage get`
/// read. One listing serves the whole batch.
async fn batch_artifacts(batch: &str) -> Result<Vec<String>, DeployError> {
    crate::cli::storage::list_object_uris(ARTIFACT_NAMESPACE, &format!("{batch}/"))
        .await
        .map_err(|error| {
            DeployError(format!(
                "cannot list stado://{ARTIFACT_NAMESPACE}/{batch}/: {error}"
            ))
        })
}

/// How many captures sit in each state, in the fixed order the report prints
/// them, plus any state word the worker used that this command does not know.
pub fn totals(states: &[CaptureState]) -> Vec<(String, usize)> {
    let mut totals: Vec<(String, usize)> = [STATE_QUEUED, STATE_RUNNING, STATE_DONE, STATE_FAILED]
        .iter()
        .map(|state| ((*state).to_string(), usize::default()))
        .collect();
    for state in states {
        match totals.iter_mut().find(|(name, _)| name == &state.state) {
            Some((_, count)) => *count += 1,
            None => totals.push((state.state.clone(), 1)),
        }
    }
    totals
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pinned_account_reaches_the_api_as_the_binding_that_keys_the_profile() {
        let request = run_request(
            "generic_browser_task",
            json!({"url": "https://x.test/"}),
            Some("oko-calendar"),
            false,
        );
        assert_eq!(request["account_id"], json!("oko-calendar"));
        // Not a fresh profile: the point of pinning is to REUSE the directory
        // that identity already has, cookies and signed-in session included.
        assert_eq!(request.get("fresh_profile"), None);
    }

    #[test]
    fn a_run_without_an_account_carries_no_binding_at_all() {
        let request = run_request("generic_browser_task", json!({}), None, false);
        assert_eq!(request.get("account_id"), None);
        assert_eq!(request["action"], json!("generic_browser_task"));
    }

    #[test]
    fn a_fresh_profile_still_names_the_account_its_new_directory_belongs_to() {
        // The API refuses `fresh_profile` without one, so the two travel together.
        let request = run_request(
            "generic_browser_task",
            json!({}),
            Some("stado-fresh-profile-1"),
            true,
        );
        assert_eq!(request["fresh_profile"], json!(true));
        assert_eq!(request["account_id"], json!("stado-fresh-profile-1"));
    }

    #[test]
    fn an_account_id_that_could_not_be_a_directory_key_is_refused_here() {
        for bad in [
            "",
            "has space",
            "slash/inside",
            "quote\"inside",
            "$(command)",
        ] {
            let said = checked_account_id(bad).unwrap_err().to_string();
            assert!(said.contains("must be 1-128 characters"), "{bad:?}: {said}");
        }
        assert_eq!(
            checked_account_id("oko-calendar.lukasz_1").unwrap(),
            "oko-calendar.lukasz_1"
        );
    }
}
