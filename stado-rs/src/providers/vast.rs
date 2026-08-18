//! Vast.ai marketplace host-listing bridge.
//!
//! Port of `stado/providers/vast/__init__.py` + `stado/providers/vast/_auth.py`.
//!
//! Wisent-compute is the renter on GCP/Azure/AWS. On Vast.ai it is the
//! HOST — we own the lab-box GPU and list it on Vast so external renters
//! use the otherwise-idle capacity when wisent-compute has nothing to
//! dispatch.
//!
//! Endpoints verified against vast-cli (github.com/vast-ai/vast-cli):
//! list_machine vast.py:8092 -> PUT /machines/create_asks/;
//! unlist__machine vast.py:8991 -> DELETE /machines/{id}/asks/.
//!
//! Auth: the `stado-vast` Skarbiec item, field `api_key`. Target machine:
//! WC_VAST_MACHINE_ID (or auto-discovered via /machines/?owner=me + hostname).
//!
//! The auto-list loop polls the wisent-compute queue + local-{hostname}
//! capacity blob; lists when idle, unlists when work appears. Existing
//! Vast rentals are NOT preempted — only NEW renters are blocked.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};

use crate::queue::{JobStorage, StorageError};

/// Python `_VAST_BASE`.
pub const VAST_BASE: &str = "https://console.vast.ai/api/v0";

/// Python `_AUTO_LIST_THREAD_RUNNING` — set true when the auto-list loop
/// starts; read by the capacity broadcast (`vast_bridge_active`, phase-3).
pub static AUTO_LIST_THREAD_RUNNING: AtomicBool = AtomicBool::new(false);

/// Vast bridge error. Python raises `VastConfigError` for config problems
/// and `RuntimeError` for HTTP failures; urllib/JSON exceptions propagate.
#[derive(Debug, thiserror::Error)]
pub enum VastError {
    /// Python `VastConfigError`.
    #[error("{0}")]
    Config(String),
    /// Python `RuntimeError` from `_request` (HTTP error status).
    #[error("{0}")]
    Api(String),
    /// Python urllib `URLError` etc. propagated from `_request`.
    #[error(transparent)]
    Http(#[from] reqwest::Error),
    /// Storage failures from the queue-state probes.
    #[error(transparent)]
    Storage(#[from] StorageError),
}

impl VastError {
    fn config(message: impl Into<String>) -> Self {
        VastError::Config(message.into())
    }

    /// The Python `type(exc).__name__` slot in "poll failed: {type}: {exc}".
    fn kind(&self) -> &'static str {
        match self {
            VastError::Config(_) => "VastConfigError",
            VastError::Api(_) => "RuntimeError",
            VastError::Http(_) => "URLError",
            VastError::Storage(_) => "StorageError",
        }
    }
}

// ---------------------------------------------------------------------------
// Credential resolution
// ---------------------------------------------------------------------------

/// Resolve the Vast API key only from Skarbiec. A missing item means that the
/// provider is unavailable; authorization and transport failures are logged
/// rather than mistaken for an absent credential.
///
/// Two channels, in this order, because the renter gate has to work on the
/// machine that is actually rented: the configured (control-plane) consumer
/// first, and the host's own agent grant when this host holds no control-plane
/// bearer. The RTX host is the whole reason — it is the fleet's only rented
/// machine, `~/.stado/control-plane-skarbiec-token` does not exist there and
/// must not, so `vast_active` was permanently false and nothing kept fleet jobs
/// off a paying renter's card.
pub async fn resolve_vast_api_key() -> String {
    let control_plane_bearer = crate::config::skarbiec_token_file();
    let control_plane_usable =
        !control_plane_bearer.is_empty() && std::path::Path::new(control_plane_bearer).is_file();
    if control_plane_usable {
        match crate::skarbiec::read_string("stado-vast", "api_key").await {
            Ok(value) => return value.unwrap_or_default(),
            Err(err) => {
                eprintln!("[vast] cannot read stado-vast/api_key from Skarbiec: {err}");
                return String::new();
            }
        }
    }
    let url = crate::config::agent_skarbiec_url();
    let consumer = crate::config::agent_skarbiec_consumer();
    let token_file = crate::config::agent_skarbiec_token_file();
    if url.is_empty() || consumer.is_empty() || !std::path::Path::new(token_file).is_file() {
        eprintln!(
            "[vast] no usable Skarbiec grant for stado-vast/api_key: no control-plane bearer at \
             {control_plane_bearer} and no agent grant configured on this host"
        );
        return String::new();
    }
    match crate::credential_store::read_string_with(url, consumer, token_file, "stado-vast", "api_key")
        .await
    {
        Ok(value) => value.unwrap_or_default(),
        Err(err) => {
            eprintln!(
                "[vast] cannot read stado-vast/api_key as {consumer} (this host's own grant): {err}"
            );
            String::new()
        }
    }
}

/// Python `vast_api_key_available`.
pub async fn vast_api_key_available() -> bool {
    !resolve_vast_api_key().await.is_empty()
}

// ---------------------------------------------------------------------------
// __init__.py — machine id resolution
// ---------------------------------------------------------------------------

/// The `WC_VAST_MACHINE_ID` env half of `_machine_id`, split out for tests:
/// stripped; empty -> None; non-int -> VastConfigError.
pub fn parse_machine_id_env(value: Option<&str>) -> Result<Option<i64>, VastError> {
    let Some(mid) = value.map(str::trim).filter(|s| !s.is_empty()) else {
        return Ok(None);
    };
    mid.parse::<i64>()
        .map(Some)
        .map_err(|_| VastError::config(format!("WC_VAST_MACHINE_ID must be int: {mid}")))
}

/// `socket.gethostname()`: the kernel hostname, not $HOSTNAME. Read from
/// /proc on Linux (the Vast host is a Linux lab box), falling back to the
/// `hostname(1)` binary elsewhere (macOS dev machines).
pub fn system_hostname() -> String {
    if let Ok(raw) = std::fs::read_to_string("/proc/sys/kernel/hostname") {
        let trimmed = raw.trim();
        if !trimmed.is_empty() {
            return trimmed.to_string();
        }
    }
    std::process::Command::new("hostname")
        .output()
        .ok()
        .filter(|out| out.status.success())
        .map(|out| String::from_utf8_lossy(&out.stdout).trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------
// __init__.py — REST client
// ---------------------------------------------------------------------------

/// Bearer-authenticated REST client against the Vast host API (Python
/// module-level `_request` + the public operations). Cheap to clone.
#[derive(Debug, Clone)]
pub struct VastClient {
    inner: Arc<Inner>,
}

#[derive(Debug)]
struct Inner {
    http: reqwest::Client,
    api_key: String,
    base_url: String,
}

/// Python `list_machine` keyword defaults as a struct.
#[derive(Debug, Clone, PartialEq)]
pub struct ListMachineParams {
    pub price_gpu: f64,
    pub price_disk: f64,
    pub price_inetu: f64,
    pub price_inetd: f64,
    pub price_min_bid: Option<f64>,
    pub min_chunk: i64,
    pub duration: Option<i64>,
}

impl Default for ListMachineParams {
    fn default() -> Self {
        ListMachineParams {
            price_gpu: 0.50,
            price_disk: 0.05,
            price_inetu: 0.01,
            price_inetd: 0.01,
            price_min_bid: None,
            min_chunk: 1,
            duration: None,
        }
    }
}

impl VastClient {
    /// Bind the client to the public Vast API with an explicit key.
    pub fn new(api_key: impl Into<String>) -> Self {
        Self::with_base_url(api_key, VAST_BASE)
    }

    /// Bind with a custom base URL (loopback mocks in tests).
    pub fn with_base_url(api_key: impl Into<String>, base_url: impl Into<String>) -> Self {
        VastClient {
            inner: Arc::new(Inner {
                http: reqwest::Client::new(),
                api_key: api_key.into(),
                base_url: base_url.into(),
            }),
        }
    }

    /// Resolve the key from `stado-vast/api_key` in Skarbiec.
    pub async fn from_env() -> Result<Self, VastError> {
        let key = resolve_vast_api_key().await;
        if key.is_empty() {
            return Err(VastError::config(
                "Skarbiec item stado-vast field api_key is required",
            ));
        }
        Ok(Self::new(key))
    }

    /// Python `_request`: Bearer-authenticated call; HTTP error statuses
    /// become a RuntimeError-style message with the body head (280 chars).
    async fn request(
        &self,
        method: &str,
        path: &str,
        body: Option<&Value>,
    ) -> Result<Value, VastError> {
        let verb = reqwest::Method::from_bytes(method.as_bytes())
            .map_err(|_| VastError::config(format!("invalid HTTP method {method:?}")))?;
        let mut request = self
            .inner
            .http
            .request(verb, format!("{}{path}", self.inner.base_url))
            .header(
                reqwest::header::AUTHORIZATION,
                format!("Bearer {}", self.inner.api_key),
            )
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .header(reqwest::header::ACCEPT, "application/json");
        if let Some(body) = body {
            request = request.body(serde_json::to_string(body).unwrap_or_else(|_| "{}".into()));
        }
        let response = request.send().await?;
        let status = response.status();
        let text = response.text().await.unwrap_or_default();
        if !status.is_success() {
            let head: String = text.chars().take(280).collect();
            return Err(VastError::Api(format!(
                "Vast.ai {method} {path} -> HTTP {}: {head}",
                status.as_u16()
            )));
        }
        // Python: json.loads(raw or "{}") — invalid JSON raises (here as
        // VastError::Api instead of a raw JSONDecodeError).
        let raw = if text.is_empty() { "{}" } else { &text };
        serde_json::from_str(raw).map_err(|err| {
            VastError::Api(format!("Vast.ai {method} {path} -> invalid JSON: {err}"))
        })
    }

    /// Resolve the machine id from the non-secret env override, else
    /// auto-discover it via `/machines/?owner=me` and hostname.
    pub async fn machine_id(&self) -> Result<i64, VastError> {
        self.machine_id_for_hostname(&system_hostname()).await
    }

    /// [`VastClient::machine_id`] with the hostname passed explicitly.
    pub async fn machine_id_for_hostname(&self, hostname: &str) -> Result<i64, VastError> {
        if let Some(mid) =
            parse_machine_id_env(std::env::var("WC_VAST_MACHINE_ID").ok().as_deref())?
        {
            return Ok(mid);
        }
        let resp = self.request("GET", "/machines/?owner=me", None).await?;
        let machines = machines_of(&resp);
        if machines.is_empty() {
            return Err(VastError::config(
                "Vast.ai /machines/?owner=me returned no machines. \
                 Register the box at https://cloud.vast.ai/host/setup first.",
            ));
        }
        for machine in &machines {
            if jstr(machine.get("hostname")).trim() == hostname {
                return machine_id_of(machine);
            }
        }
        if machines.len() == 1 {
            return machine_id_of(machines[0]);
        }
        let candidates: Vec<String> = machines
            .iter()
            .map(|m| {
                format!(
                    "{}={}",
                    py_value_str(m.get("id")),
                    py_value_str(m.get("hostname"))
                )
            })
            .collect();
        Err(VastError::config(format!(
            "Vast.ai returned {} machines and hostname '{hostname}' did not match any. \
             Set WC_VAST_MACHINE_ID explicitly. Candidates: {}",
            machines.len(),
            candidates.join(", ")
        )))
    }

    /// Python `list_machine`: list the configured machine on the
    /// marketplace at the given prices. PUT /api/v0/machines/create_asks/
    /// with the machine id from WC_VAST_MACHINE_ID / auto-discovery.
    pub async fn list_machine(&self, params: &ListMachineParams) -> Result<Value, VastError> {
        let mid = self.machine_id().await?;
        self.list_machine_with_id(mid, params).await
    }

    /// [`VastClient::list_machine`] with the machine id resolved by the
    /// caller (tests; the auto-list loop's startup sync).
    pub async fn list_machine_with_id(
        &self,
        machine_id: i64,
        params: &ListMachineParams,
    ) -> Result<Value, VastError> {
        let mut body = json!({
            "machine": machine_id,
            "price_gpu": params.price_gpu,
            "price_disk": params.price_disk,
            "price_inetu": params.price_inetu,
            "price_inetd": params.price_inetd,
            "min_chunk": params.min_chunk,
        });
        if let Some(price_min_bid) = params.price_min_bid {
            body["price_min_bid"] = json!(price_min_bid);
        }
        if let Some(duration) = params.duration {
            body["duration"] = json!(duration);
        }
        self.request("PUT", "/machines/create_asks/", Some(&body))
            .await
    }

    /// Python `unlist_machine`: remove every active offer from the
    /// configured machine. DELETE /api/v0/machines/{id}/asks/. Does NOT
    /// terminate existing rentals — those run until the renter releases
    /// them.
    pub async fn unlist_machine(&self) -> Result<Value, VastError> {
        let mid = self.machine_id().await?;
        self.unlist_machine_with_id(mid).await
    }

    /// [`VastClient::unlist_machine`] with the machine id resolved.
    pub async fn unlist_machine_with_id(&self, machine_id: i64) -> Result<Value, VastError> {
        self.request("DELETE", &format!("/machines/{machine_id}/asks/"), None)
            .await
    }

    /// Python `machine_status`: the current Vast.ai view of our machine
    /// (current_rentals, listed_status, etc.), or an explicit not-found
    /// record when /machines/?owner=me doesn't include it.
    pub async fn machine_status(&self) -> Result<Value, VastError> {
        let mid = self.machine_id().await?;
        self.machine_status_with_id(mid).await
    }

    /// [`VastClient::machine_status`] with the machine id resolved.
    pub async fn machine_status_with_id(&self, machine_id: i64) -> Result<Value, VastError> {
        let resp = self.request("GET", "/machines/?owner=me", None).await?;
        for machine in machines_of(&resp) {
            if machine.get("id").and_then(json_int) == Some(machine_id) {
                return Ok(machine.clone());
            }
        }
        Ok(json!({"id": machine_id, "error": "not found in /machines/?owner=me response"}))
    }
}

/// Python `resp.get("machines") or resp.get("results") or []`: the first
/// truthy array wins (an empty "machines" list falls through to "results").
fn machines_of(resp: &Value) -> Vec<&Value> {
    for key in ["machines", "results"] {
        if let Some(array) = resp.get(key).and_then(Value::as_array) {
            if !array.is_empty() {
                return array.iter().collect();
            }
        }
    }
    Vec::new()
}

/// Python `int(m["id"])`: numbers pass, numeric strings parse.
fn machine_id_of(machine: &Value) -> Result<i64, VastError> {
    machine
        .get("id")
        .and_then(json_int)
        .ok_or_else(|| VastError::config(format!("Vast.ai machine lacks an int id: {machine}")))
}

/// Python `int(value)` for JSON scalars (strings included).
fn json_int(value: &Value) -> Option<i64> {
    match value {
        Value::Number(n) => n.as_i64(),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
}

/// Python `str(value.get(key) or "")` for string-ish fields.
fn jstr(value: Option<&Value>) -> String {
    match value {
        Some(Value::String(s)) => s.clone(),
        _ => String::new(),
    }
}

/// Python f-string rendering of a JSON scalar (None/True/False included).
fn py_value_str(value: Option<&Value>) -> String {
    match value {
        None | Some(Value::Null) => "None".to_string(),
        Some(Value::Bool(b)) => if *b { "True" } else { "False" }.to_string(),
        Some(Value::Number(n)) => n.to_string(),
        Some(Value::String(s)) => s.clone(),
        Some(other) => other.to_string(),
    }
}

/// Python str(float): integral floats keep one decimal ("3600.0").
fn py_float(value: f64) -> String {
    if value.is_finite() && value.fract() == 0.0 {
        format!("{value:.1}")
    } else {
        format!("{value}")
    }
}

// ---------------------------------------------------------------------------
// __init__.py — queue-state probe (pure decision inputs)
// ---------------------------------------------------------------------------

/// Python `_is_stado_busy` result.
#[derive(Debug, Clone, PartialEq)]
pub struct BusyState {
    pub queued: usize,
    pub running_here: usize,
    pub free_vram_gb: Option<f64>,
    pub idle: bool,
}

/// Python `_is_stado_busy`: busy = queue not empty OR any running/ blob has
/// instance_ref referencing this hostname. (The earlier impl read
/// claimed_this_loop — a per-iter counter, ~always 0 — and missed in-flight
/// work.) Corrupt or vanished blobs are skipped, like Python's blanket
/// `except (NotFound, Exception): continue`.
pub async fn is_stado_busy(store: &JobStorage, hostname: &str) -> Result<BusyState, StorageError> {
    // Python list_blobs(prefix="queue/", max_results=2) — capped at 2.
    let queued = store.list_paths("queue/", 2).await?.len();
    let mut running_here = 0;
    for path in store.list_paths("running/", 0).await? {
        let Ok(Some(text)) = store.download_text(&path).await else {
            continue;
        };
        let Ok(doc) = serde_json::from_str::<Value>(&text) else {
            continue;
        };
        let instance_ref = doc
            .get("instance_ref")
            .and_then(Value::as_str)
            .unwrap_or("");
        if !hostname.is_empty() && instance_ref.contains(hostname) {
            running_here += 1;
        }
    }
    let free_vram_gb = match store
        .download_text(&format!("capacity/local-{hostname}.json"))
        .await
    {
        Ok(Some(text)) => serde_json::from_str::<Value>(&text)
            .ok()
            .and_then(|doc| doc.get("free_vram_gb").and_then(Value::as_f64)),
        _ => None,
    };
    Ok(BusyState {
        queued,
        running_here,
        free_vram_gb,
        idle: queued == 0 && running_here == 0,
    })
}

// ---------------------------------------------------------------------------
// __init__.py — auto-list decision (pure) + daemon loop
// ---------------------------------------------------------------------------

/// What the auto-list loop should do this iteration (pure decision; the
/// loop applies it and logs the Python messages).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum AutoListAction {
    /// Idle long enough and not listed: list the machine.
    List { idle_dur_s: i64 },
    /// Idle but still inside the window (or already listed): keep waiting.
    IdleCountdown { idle_dur_s: i64 },
    /// Work appeared while listed: unlist immediately.
    Unlist,
    /// Offer gone, work queued, near-zero free VRAM: a Vast rental is on
    /// the GPU; wisent-compute claims as soon as the renter releases.
    WaitingForRental { free_vram_gb: f64 },
    /// Busy and not listed: nothing to do.
    BusyNotListed,
}

/// The pure idle/unlist decision from `auto_list_loop`.
pub fn decide_action(
    listed: bool,
    state: &BusyState,
    idle_dur_s: i64,
    idle_window_s: i64,
) -> AutoListAction {
    if state.idle {
        if idle_dur_s >= idle_window_s && !listed {
            AutoListAction::List { idle_dur_s }
        } else {
            AutoListAction::IdleCountdown { idle_dur_s }
        }
    } else if listed {
        AutoListAction::Unlist
    } else if state.queued > 0 && state.free_vram_gb.is_some_and(|free| free < 10.0) {
        AutoListAction::WaitingForRental {
            free_vram_gb: state.free_vram_gb.unwrap_or(0.0),
        }
    } else {
        AutoListAction::BusyNotListed
    }
}

/// Python `auto_list_loop` keyword arguments as a struct.
#[derive(Debug, Clone, PartialEq)]
pub struct AutoListParams {
    /// Wisent-compute must be idle this many consecutive seconds before
    /// listing (default 300).
    pub idle_window_s: i64,
    /// Polling interval against the wisent-compute bucket (default 10s —
    /// short enough to catch transient queue states).
    pub poll_interval_s: u64,
    /// Per-GPU-hour rental price USD when we list (default 0.50).
    pub price_gpu: f64,
    /// Caps the maximum length of any rental Vast can hand out from this
    /// offer (PUT /machines/create_asks/ duration field, vast-cli
    /// vast.py:8092). With duration_s=3600 the worst-case wait for a
    /// wisent-compute job behind an active Vast rental is one hour; None
    /// leaves the offer open-ended. Default 15768000 (half a year);
    /// WC_VAST_MAX_DURATION_S env wins (cli.py uneditable).
    pub duration_s: Option<i64>,
    /// Print the toggle decisions without calling the Vast API.
    pub dry_run: bool,
}

impl Default for AutoListParams {
    fn default() -> Self {
        AutoListParams {
            idle_window_s: 300,
            poll_interval_s: 10,
            price_gpu: 0.50,
            duration_s: Some(15768000),
            dry_run: false,
        }
    }
}

/// Python `auto_list_loop` daemon: poll wisent-compute state, toggle the
/// Vast.ai listing. Lists the machine when wisent-compute has been idle
/// for `idle_window_s` consecutive seconds; unlists the moment any work
/// shows up. Existing Vast rentals are NOT touched. Runs forever, like
/// the Python loop (the CLI drives it as a daemon thread).
pub async fn auto_list_loop(
    client: &VastClient,
    store: &JobStorage,
    hostname: &str,
    params: AutoListParams,
    mut log: impl FnMut(&str),
) -> Result<(), VastError> {
    // WC_VAST_MAX_DURATION_S env wins (cli.py uneditable).
    let mut duration_s = params.duration_s;
    if let Ok(raw) = std::env::var("WC_VAST_MAX_DURATION_S") {
        if !raw.is_empty() {
            duration_s = Some(raw.trim().parse::<i64>().map_err(|_| {
                VastError::config(format!("WC_VAST_MAX_DURATION_S must be int: {raw}"))
            })?);
        }
    }
    AUTO_LIST_THREAD_RUNNING.store(true, Ordering::SeqCst);
    let mut idle_since: Option<std::time::Instant> = None;
    // Startup sync: take over any pre-existing listing (manual host-UI
    // placement or an earlier bridge run) so the loop can unlist it when
    // wisent-compute work arrives. Normalize price + duration to the
    // bridge's configured values.
    let mut listed = false;
    match client.machine_status().await {
        Ok(status) => {
            let cur_price = status
                .get("listed_gpu_cost")
                .cloned()
                .unwrap_or(Value::Null);
            if let Some(current) = cur_price.as_f64().filter(|c| *c > 0.0) {
                listed = true;
                log(&format!(
                    "startup: listed at ${}/h on machine_id={}",
                    py_value_str(Some(&cur_price)),
                    py_value_str(status.get("id"))
                ));
                if (current - params.price_gpu).abs() > 0.01 && !params.dry_run {
                    let normalize = async {
                        client.unlist_machine().await?;
                        client
                            .list_machine(&ListMachineParams {
                                price_gpu: params.price_gpu,
                                duration: duration_s,
                                ..ListMachineParams::default()
                            })
                            .await
                    };
                    match normalize.await {
                        Ok(_) => log(&format!(
                            "normalized ${}/h -> ${}/h",
                            py_value_str(Some(&cur_price)),
                            py_float(params.price_gpu)
                        )),
                        Err(exc) => log(&format!("normalize failed: {exc}")),
                    }
                }
            } else {
                log("startup: not currently listed");
            }
        }
        Err(exc) => log(&format!("startup probe failed: {exc}")),
    }
    loop {
        if let Some(reservation) = crate::inference::reservation::active() {
            idle_since = None;
            if listed {
                if params.dry_run {
                    log(&format!(
                        "DRY-RUN would unlist for inference reservation '{}'",
                        reservation.deployment
                    ));
                } else {
                    match client.unlist_machine().await {
                        Ok(_) => {
                            listed = false;
                            log(&format!(
                                "unlisted for inference reservation '{}'",
                                reservation.deployment
                            ));
                        }
                        Err(exc) => log(&format!(
                            "unlist for inference reservation '{}' failed: {exc}",
                            reservation.deployment
                        )),
                    }
                }
            }
            tokio::time::sleep(Duration::from_secs(params.poll_interval_s)).await;
            continue;
        }
        let state = match is_stado_busy(store, hostname).await {
            Ok(state) => state,
            Err(exc) => {
                let wrapped = VastError::Storage(exc);
                log(&format!("poll failed: {}: {wrapped}", wrapped.kind()));
                tokio::time::sleep(Duration::from_secs(params.poll_interval_s)).await;
                continue;
            }
        };
        if state.idle {
            let now = std::time::Instant::now();
            let since = idle_since.get_or_insert(now);
            let idle_dur = now.duration_since(*since).as_secs() as i64;
            match decide_action(listed, &state, idle_dur, params.idle_window_s) {
                AutoListAction::List { idle_dur_s } => {
                    if params.dry_run {
                        log(&format!("DRY-RUN would list (idle {idle_dur_s}s)"));
                    } else {
                        match client
                            .list_machine(&ListMachineParams {
                                price_gpu: params.price_gpu,
                                duration: duration_s,
                                ..ListMachineParams::default()
                            })
                            .await
                        {
                            Ok(_) => {
                                listed = true;
                                log(&format!(
                                    "LISTED ({idle_dur_s}s idle, ${}/h, dur={}s)",
                                    py_float(params.price_gpu),
                                    duration_s
                                        .map_or_else(|| "None".to_string(), |d| d.to_string())
                                ));
                            }
                            Err(exc) => log(&format!("list failed: {exc}")),
                        }
                    }
                }
                AutoListAction::IdleCountdown { idle_dur_s } => log(&format!(
                    "idle {idle_dur_s}s/{}s (listed={})",
                    params.idle_window_s,
                    if listed { "True" } else { "False" }
                )),
                _ => unreachable!("idle state only yields List/IdleCountdown"),
            }
        } else {
            idle_since = None;
            if decide_action(listed, &state, 0, params.idle_window_s) == AutoListAction::Unlist {
                if params.dry_run {
                    log(&format!(
                        "DRY-RUN would unlist (queued={}, running_here={})",
                        state.queued, state.running_here
                    ));
                } else {
                    match client.unlist_machine().await {
                        Ok(_) => {
                            listed = false;
                            log(&format!(
                                "UNLISTED (queued={}, running_here={})",
                                state.queued, state.running_here
                            ));
                        }
                        Err(exc) => log(&format!("unlist failed: {exc}")),
                    }
                }
            }
            // Visibility for the "wait for renter to finish" path: if the
            // offer is already gone AND wisent-compute has queued work AND
            // the box has near-zero free VRAM, that means a Vast rental is
            // still on the GPU and the wisent-compute claim loop is going
            // to sit idle until the renter releases (or hits the duration
            // cap). Explicit log so the operator can tell this state apart
            // from a plain dead-agent state.
            match decide_action(listed, &state, 0, params.idle_window_s) {
                AutoListAction::WaitingForRental { free_vram_gb } => log(&format!(
                    "waiting for Vast rental to finish (queued={}, free_vram_gb={}); \
                     wisent-compute jobs claim as soon as renter releases",
                    state.queued,
                    py_float(free_vram_gb)
                )),
                AutoListAction::BusyNotListed => log(&format!(
                    "busy (queued={}, running_here={}); not listed",
                    state.queued, state.running_here
                )),
                _ => {}
            }
        }
        tokio::time::sleep(Duration::from_secs(params.poll_interval_s)).await;
    }
}

/// The monitor snapshot capacity-read helper shared with the CLI: parse
/// `capacity/local-{hostname}.json`, mirroring the Python error records.
pub async fn read_capacity_snapshot(store: &JobStorage, hostname: &str) -> Value {
    let path = format!("capacity/local-{hostname}.json");
    match store.download_text(&path).await {
        Ok(Some(text)) => serde_json::from_str(&text)
            .unwrap_or_else(|exc| json!({"error": format!("JSONDecodeError: {exc}")})),
        Ok(None) => json!({"error": format!("{path} not found")}),
        Err(exc) => json!({"error": format!("{exc}")}),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::queue::local_file::LocalBackend;
    use crate::testutil::{http_response, mock_http};

    fn busy(queued: usize, running_here: usize, free_vram_gb: Option<f64>) -> BusyState {
        BusyState {
            queued,
            running_here,
            free_vram_gb,
            idle: queued == 0 && running_here == 0,
        }
    }

    #[test]
    fn env_parsing_helpers() {
        assert_eq!(parse_machine_id_env(Some(" 42 ")).unwrap(), Some(42));
        assert_eq!(parse_machine_id_env(Some("")).unwrap(), None);
        assert_eq!(parse_machine_id_env(None).unwrap(), None);
        let err = parse_machine_id_env(Some("abc")).unwrap_err();
        assert_eq!(err.to_string(), "WC_VAST_MACHINE_ID must be int: abc");
    }

    #[test]
    fn decide_action_covers_all_branches() {
        // Idle past window, not listed -> List.
        assert_eq!(
            decide_action(false, &busy(0, 0, Some(20.0)), 300, 300),
            AutoListAction::List { idle_dur_s: 300 }
        );
        // Idle inside window -> countdown.
        assert_eq!(
            decide_action(false, &busy(0, 0, None), 120, 300),
            AutoListAction::IdleCountdown { idle_dur_s: 120 }
        );
        // Idle past window but already listed -> countdown (no double-list).
        assert_eq!(
            decide_action(true, &busy(0, 0, None), 600, 300),
            AutoListAction::IdleCountdown { idle_dur_s: 600 }
        );
        // Busy while listed -> Unlist.
        assert_eq!(
            decide_action(true, &busy(1, 0, Some(0.0)), 0, 300),
            AutoListAction::Unlist
        );
        assert_eq!(
            decide_action(true, &busy(0, 1, None), 0, 300),
            AutoListAction::Unlist
        );
        // Busy, unlisted, queued work, near-zero VRAM -> waiting for renter.
        assert_eq!(
            decide_action(false, &busy(2, 0, Some(3.5)), 0, 300),
            AutoListAction::WaitingForRental { free_vram_gb: 3.5 }
        );
        // VRAM unknown or >= 10 -> plain busy.
        assert_eq!(
            decide_action(false, &busy(2, 0, None), 0, 300),
            AutoListAction::BusyNotListed
        );
        assert_eq!(
            decide_action(false, &busy(2, 0, Some(10.0)), 0, 300),
            AutoListAction::BusyNotListed
        );
        assert_eq!(
            decide_action(false, &busy(0, 1, Some(0.0)), 0, 300),
            AutoListAction::BusyNotListed
        );
    }

    #[tokio::test]
    async fn is_stado_busy_reads_queue_running_and_capacity() {
        let dir = tempfile::tempdir().unwrap();
        let backend = LocalBackend::new(dir.path().to_str().unwrap()).unwrap();
        let store = JobStorage::with_backend(Arc::new(backend), "local");

        // Empty deployment -> idle.
        let state = is_stado_busy(&store, "myhost").await.unwrap();
        assert_eq!(state, busy(0, 0, None));

        // A queued job breaks idleness; a corrupt running blob is skipped.
        store.upload_text("queue/j1.json", "{}").await.unwrap();
        store
            .upload_text("running/broken.json", "{not json")
            .await
            .unwrap();
        // A running job on another host does not count; one on this host does.
        store
            .upload_text(
                "running/j2.json",
                r#"{"instance_ref": "wisent-a@us-central1-b"}"#,
            )
            .await
            .unwrap();
        store
            .upload_text("running/j3.json", r#"{"instance_ref": "vast-99-myhost-2"}"#)
            .await
            .unwrap();
        store
            .upload_text("capacity/local-myhost.json", r#"{"free_vram_gb": 3.5}"#)
            .await
            .unwrap();
        let state = is_stado_busy(&store, "myhost").await.unwrap();
        assert_eq!(state, busy(1, 1, Some(3.5)));
        assert!(!state.idle);

        // Hostname substring semantics, like Python `hostname in instance_ref`.
        let state = is_stado_busy(&store, "myhost-2").await.unwrap();
        assert_eq!(state.running_here, 1);
        let state = is_stado_busy(&store, "otherhost").await.unwrap();
        assert_eq!(state.running_here, 0);
        // No capacity blob for another hostname.
        assert_eq!(state.free_vram_gb, None);
    }

    async fn vast_for(responses: Vec<String>) -> (crate::testutil::MockHttp, VastClient) {
        let server = mock_http(responses).await;
        let client = VastClient::with_base_url("vastkey", &server.base_url);
        (server, client)
    }

    #[tokio::test]
    async fn machine_id_discovery_prefers_hostname_then_single() {
        // Hostname match among several machines.
        let (server, client) = vast_for(vec![http_response(
            200,
            "OK",
            r#"{"machines": [{"id": 11, "hostname": "other"}, {"id": 22, "hostname": "labbox"}]}"#,
        )])
        .await;
        assert_eq!(client.machine_id_for_hostname("labbox").await.unwrap(), 22);
        let requests = server.requests.lock().unwrap().clone();
        assert!(
            requests[0].starts_with("GET /machines/?owner=me "),
            "{}",
            requests[0]
        );
        assert!(
            requests[0].contains("authorization: Bearer vastkey"),
            "{}",
            requests[0]
        );
        server.stop();

        // No hostname match, exactly one machine -> take it.
        let (server, client) = vast_for(vec![http_response(
            200,
            "OK",
            r#"{"results": [{"id": 33, "hostname": "whatever"}]}"#,
        )])
        .await;
        assert_eq!(client.machine_id_for_hostname("labbox").await.unwrap(), 33);
        server.stop();

        // Several machines, no match -> candidates error.
        let (server, client) = vast_for(vec![http_response(
            200,
            "OK",
            r#"{"machines": [{"id": 11, "hostname": "a"}, {"id": 22}]}"#,
        )])
        .await;
        let err = client.machine_id_for_hostname("labbox").await.unwrap_err();
        assert!(matches!(err, VastError::Config(_)), "{err:?}");
        assert_eq!(
            err.to_string(),
            "Vast.ai returned 2 machines and hostname 'labbox' did not match any. \
             Set WC_VAST_MACHINE_ID explicitly. Candidates: 11=a, 22=None"
        );
        server.stop();

        // No machines -> register-first error. An empty "machines" list
        // falls through to "results", like Python's `or` chain.
        let (server, client) =
            vast_for(vec![http_response(200, "OK", r#"{"machines": []}"#)]).await;
        let err = client.machine_id_for_hostname("labbox").await.unwrap_err();
        assert!(err.to_string().contains("returned no machines"), "{err}");
        server.stop();
    }

    #[tokio::test]
    async fn list_unlist_and_status_bodies() {
        let (server, client) = vast_for(vec![
            http_response(200, "OK", r#"{"success": true}"#),
            http_response(200, "OK", r#"{"success": true}"#),
            http_response(
                200,
                "OK",
                r#"{"machines": [{"id": 42, "listed_gpu_cost": 0.5, "current_rentals": []}]}"#,
            ),
        ])
        .await;
        let params = ListMachineParams {
            price_gpu: 0.75,
            price_min_bid: Some(0.3),
            duration: Some(3600),
            ..ListMachineParams::default()
        };
        client.list_machine_with_id(42, &params).await.unwrap();
        client.unlist_machine_with_id(42).await.unwrap();
        let status = client.machine_status_with_id(42).await.unwrap();
        assert_eq!(status["listed_gpu_cost"], json!(0.5));

        let requests = server.requests.lock().unwrap().clone();
        assert!(
            requests[0].starts_with("PUT /machines/create_asks/ "),
            "{}",
            requests[0]
        );
        assert!(
            requests[0].ends_with(
                r#"{"machine":42,"price_gpu":0.75,"price_disk":0.05,"price_inetu":0.01,"price_inetd":0.01,"min_chunk":1,"price_min_bid":0.3,"duration":3600}"#
            ),
            "{}",
            requests[0]
        );
        assert!(
            requests[1].starts_with("DELETE /machines/42/asks/ "),
            "{}",
            requests[1]
        );
        server.stop();

        // Machine absent from owner listing -> explicit not-found record.
        let (server, client) = vast_for(vec![http_response(
            200,
            "OK",
            r#"{"machines": [{"id": 1}]}"#,
        )])
        .await;
        let status = client.machine_status_with_id(42).await.unwrap();
        assert_eq!(
            status["error"],
            json!("not found in /machines/?owner=me response")
        );
        server.stop();
    }

    #[tokio::test]
    async fn http_errors_become_runtime_error_style_messages() {
        let body = format!("{{\"error\": \"{}\"}}", "x".repeat(400));
        let (server, client) = vast_for(vec![http_response(403, "Forbidden", &body)]).await;
        let err = client.unlist_machine_with_id(42).await.unwrap_err();
        let VastError::Api(message) = err else {
            panic!("expected Api error: {err:?}")
        };
        // Method + path + status + 280-char body head.
        assert!(
            message.starts_with("Vast.ai DELETE /machines/42/asks/ -> HTTP 403: "),
            "{message}"
        );
        assert_eq!(
            message.len(),
            "Vast.ai DELETE /machines/42/asks/ -> HTTP 403: ".len() + 280
        );
        server.stop();
    }

    #[tokio::test]
    async fn read_capacity_snapshot_error_records() {
        let dir = tempfile::tempdir().unwrap();
        let backend = LocalBackend::new(dir.path().to_str().unwrap()).unwrap();
        let store = JobStorage::with_backend(Arc::new(backend), "local");

        // Missing blob -> not-found record.
        let cap = read_capacity_snapshot(&store, "h1").await;
        assert_eq!(cap["error"], json!("capacity/local-h1.json not found"));

        // Valid blob passes through.
        store
            .upload_text("capacity/local-h1.json", r#"{"free_vram_gb": 0}"#)
            .await
            .unwrap();
        let cap = read_capacity_snapshot(&store, "h1").await;
        assert_eq!(cap["free_vram_gb"], json!(0));

        // Corrupt JSON -> error record.
        store
            .upload_text("capacity/local-h2.json", "{nope")
            .await
            .unwrap();
        let cap = read_capacity_snapshot(&store, "h2").await;
        assert!(
            cap["error"]
                .as_str()
                .unwrap()
                .starts_with("JSONDecodeError: "),
            "{cap}"
        );
    }

    #[test]
    fn float_and_value_formatting() {
        assert_eq!(py_float(0.5), "0.5");
        assert_eq!(py_float(3600.0), "3600.0");
        assert_eq!(py_value_str(Some(&json!(0.5))), "0.5");
        assert_eq!(py_value_str(Some(&json!(null))), "None");
        assert_eq!(py_value_str(None), "None");
        assert_eq!(py_value_str(Some(&json!(true))), "True");
    }
}
