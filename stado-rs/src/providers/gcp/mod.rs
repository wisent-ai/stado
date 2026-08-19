//! GCP provider: GCE instance lifecycle.
//!
//! Port of `stado/providers/gcp/__init__.py`. The Python module uses the
//! google-cloud-compute SDK (`compute_v1.InstancesClient`, gRPC); this port
//! talks to the GCE REST API v1 (`https://compute.googleapis.com/compute/v1`)
//! with gcp_auth (cloud-platform scope), the same auth pattern as
//! [`crate::queue::gcs`]. Long-running insert operations are polled via
//! `GET .../zones/{zone}/operations/{op}` until DONE — the Rust equivalent
//! of the Python SDK's `op.result()`.
//!
//! Cross-instance stockout/quota caches live in [`stockout`].
//!
//! Deviation: Python's `GCPProvider()` constructor eagerly builds the SDK
//! client (failing on missing ADC at `get_provider` time). Here
//! [`GcpProvider::from_env`] is lazy — credentials and the JobStorage are
//! resolved on the first API call, so `get_provider("gcp")` stays a cheap,
//! sync factory. Failures surface on the first method call instead.

pub mod inventory;
pub mod stockout;

use std::collections::{BTreeMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde_json::{json, Value};
use tokio::sync::OnceCell;

use crate::config;
use crate::queue::JobStorage;

use super::{Provider, ProviderError};

/// GCE REST API v1 base.
pub const COMPUTE_API_BASE: &str = "https://compute.googleapis.com/compute/v1";
/// OAuth scope matching the Python google-cloud-compute client.
const CLOUD_PLATFORM_SCOPE: &str = "https://www.googleapis.com/auth/cloud-platform";

/// Python `_log`.
fn log(msg: &str) {
    eprintln!("[gcp] {msg}");
}

/// Python f-string rendering of a bool.
fn py_bool(value: bool) -> &'static str {
    if value {
        "True"
    } else {
        "False"
    }
}

/// GCE transport/API error. The `Api` message embeds the error codes
/// (`error.errors[].reason` for synchronous failures, the LRO
/// `error.errors[].code` for operation failures) plus the API message text
/// so the Python substring classification ("QUOTA_EXCEEDED",
/// "ZONE_RESOURCE_POOL_EXHAUSTED", "STOCKOUT", "already exists") works on
/// `error.to_string()` exactly like it did on `str(exc)` from the SDK.
#[derive(Debug, thiserror::Error)]
pub enum GceError {
    /// Python: ADC lookup failure at client construction.
    #[error("no GCP credentials found for the GCE compute API: {0}")]
    Auth(String),
    /// Transport failure (Python: SSL EOF / RetryError from the SDK).
    #[error(transparent)]
    Http(#[from] reqwest::Error),
    /// Non-2xx response or failed LRO; message carries codes + body text.
    #[error("{0}")]
    Api(String),
}

/// Bearer-authenticated GCE REST v1 client. Cheap to clone.
#[derive(Clone)]
pub struct GceClient {
    inner: Arc<Inner>,
}

struct Inner {
    http: reqwest::Client,
    project: String,
    base_url: String,
    auth: Option<Arc<dyn gcp_auth::TokenProvider>>,
    /// Delay between LRO polls. Python `op.result()` uses the SDK default
    /// (~1s); tests shrink this to milliseconds.
    poll_interval: Duration,
}

impl GceClient {
    /// Bind the client to the public GCE API, resolving GCP credentials
    /// (cloud-platform scope). No credentials is a hard error (same as the
    /// Python SDK client construction).
    pub async fn new(project: &str) -> Result<Self, GceError> {
        let auth = crate::skarbiec::gcp_provider()
            .await
            .map_err(|err| GceError::Auth(err.to_string()))?;
        Ok(Self::assemble(
            project,
            COMPUTE_API_BASE,
            Some(auth),
            Duration::from_secs(1),
        ))
    }

    /// Bind to an explicit base URL without credentials (loopback mocks in
    /// tests) and with a near-zero LRO poll interval.

    fn assemble(
        project: &str,
        base_url: &str,
        auth: Option<Arc<dyn gcp_auth::TokenProvider>>,
        poll_interval: Duration,
    ) -> Self {
        GceClient {
            inner: Arc::new(Inner {
                http: reqwest::Client::new(),
                project: project.to_string(),
                base_url: base_url.trim_end_matches('/').to_string(),
                auth,
                poll_interval,
            }),
        }
    }

    /// The project this client is bound to (Python `self.project`).
    pub fn project(&self) -> &str {
        &self.inner.project
    }

    /// Fresh (cached by gcp_auth until expiry) bearer token; None in tests.
    async fn token(&self) -> Result<Option<String>, GceError> {
        let Some(auth) = &self.inner.auth else {
            return Ok(None);
        };
        let token = auth
            .token(&[CLOUD_PLATFORM_SCOPE])
            .await
            .map_err(|err| GceError::Auth(err.to_string()))?;
        Ok(Some(format!("Bearer {}", token.as_str())))
    }

    /// Send one request; the raw response is returned unchecked so callers
    /// can apply their own status handling (404 allowances).
    async fn send(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<&Value>,
    ) -> Result<reqwest::Response, GceError> {
        let mut request = self
            .inner
            .http
            .request(method, format!("{}{path}", self.inner.base_url))
            .header(reqwest::header::ACCEPT, "application/json");
        if let Some(token) = self.token().await? {
            request = request.header(reqwest::header::AUTHORIZATION, token);
        }
        if let Some(body) = body {
            request = request
                .header(reqwest::header::CONTENT_TYPE, "application/json")
                .body(serde_json::to_string(body).unwrap_or_else(|_| "{}".into()));
        }
        Ok(request.send().await?)
    }

    /// Lift a non-2xx response into [`GceError::Api`], embedding the
    /// `error.errors[].reason|code` values and the message text so Python's
    /// substring classification keeps working.
    async fn api_error(response: reqwest::Response, desc: &str) -> GceError {
        let status = response.status().as_u16();
        let text = response.text().await.unwrap_or_default();
        let parsed = serde_json::from_str::<Value>(&text).unwrap_or(Value::Null);
        let error = parsed.get("error").cloned().unwrap_or(Value::Null);
        let mut codes = Vec::new();
        if let Some(entries) = error.get("errors").and_then(Value::as_array) {
            for entry in entries {
                // GCE uses `reason` on synchronous error bodies and `code`
                // on LRO error bodies; carry both forms.
                for key in ["reason", "code"] {
                    if let Some(code) = entry.get(key).and_then(Value::as_str) {
                        codes.push(code.to_string());
                        break;
                    }
                }
            }
        }
        let message = error.get("message").and_then(Value::as_str).unwrap_or("");
        let detail = if message.is_empty() && codes.is_empty() {
            text.chars().take(280).collect::<String>()
        } else {
            format!("{} {message}", codes.join(" ")).trim().to_string()
        };
        GceError::Api(format!("GCE {desc} -> HTTP {status}: {detail}"))
    }

    /// GET a JSON resource; non-2xx is an [`GceError::Api`].
    pub async fn get(&self, path: &str, desc: &str) -> Result<Value, GceError> {
        let response = self.send(reqwest::Method::GET, path, None).await?;
        if !response.status().is_success() {
            return Err(Self::api_error(response, desc).await);
        }
        let text = response.text().await.unwrap_or_default();
        serde_json::from_str(&text)
            .map_err(|err| GceError::Api(format!("GCE {desc} -> invalid JSON: {err}")))
    }

    /// GET that maps 404 to `None` (Python's `except NotFound`).
    pub async fn get_allow_404(&self, path: &str, desc: &str) -> Result<Option<Value>, GceError> {
        let response = self.send(reqwest::Method::GET, path, None).await?;
        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        if !response.status().is_success() {
            return Err(Self::api_error(response, desc).await);
        }
        let text = response.text().await.unwrap_or_default();
        serde_json::from_str(&text)
            .map(Some)
            .map_err(|err| GceError::Api(format!("GCE {desc} -> invalid JSON: {err}")))
    }

    /// POST a JSON body, returning the parsed response (an Operation for
    /// instance inserts).
    pub async fn post(&self, path: &str, body: &Value, desc: &str) -> Result<Value, GceError> {
        let response = self.send(reqwest::Method::POST, path, Some(body)).await?;
        if !response.status().is_success() {
            return Err(Self::api_error(response, desc).await);
        }
        let text = response.text().await.unwrap_or_default();
        serde_json::from_str(&text)
            .map_err(|err| GceError::Api(format!("GCE {desc} -> invalid JSON: {err}")))
    }

    /// DELETE a resource; `false` on 404 (idempotent NotFound). Does NOT
    /// wait for the returned operation — Python's `client.delete` call
    /// never invokes `op.result()` either.
    pub async fn delete_allow_404(&self, path: &str, desc: &str) -> Result<bool, GceError> {
        let response = self.send(reqwest::Method::DELETE, path, None).await?;
        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(false);
        }
        if !response.status().is_success() {
            return Err(Self::api_error(response, desc).await);
        }
        Ok(true)
    }

    /// Poll a zone operation until DONE (Python `op.result()`). When the
    /// operation completes with an error, the `error.errors[].code` values
    /// (e.g. QUOTA_EXCEEDED, ZONE_RESOURCE_POOL_EXHAUSTED) are surfaced in
    /// the [`GceError::Api`] message — this is where Python's substring
    /// classification reads them from.
    pub async fn wait_zone_operation(
        &self,
        zone: &str,
        operation: &str,
        desc: &str,
    ) -> Result<(), GceError> {
        let path = format!(
            "/projects/{}/zones/{zone}/operations/{operation}",
            self.project()
        );
        loop {
            let op = self
                .get(&path, &format!("get operation {operation}"))
                .await?;
            if op.get("status").and_then(Value::as_str) == Some("DONE") {
                if let Some(error) = op.get("error") {
                    let mut codes = Vec::new();
                    let mut messages = Vec::new();
                    if let Some(entries) = error.get("errors").and_then(Value::as_array) {
                        for entry in entries {
                            if let Some(code) = entry.get("code").and_then(Value::as_str) {
                                codes.push(code.to_string());
                            }
                            if let Some(message) = entry.get("message").and_then(Value::as_str) {
                                messages.push(message.to_string());
                            }
                        }
                    }
                    return Err(GceError::Api(format!(
                        "GCE {desc} operation {operation} failed: {} {}",
                        codes.join(" "),
                        messages.join("; ")
                    )));
                }
                return Ok(());
            }
            tokio::time::sleep(self.inner.poll_interval).await;
        }
    }

    /// `GET .../zones/{zone}/instances/{name}` status, or None when the
    /// instance does not exist.
    async fn instance_status(&self, zone: &str, name: &str) -> Result<Option<String>, GceError> {
        let path = format!("/projects/{}/zones/{zone}/instances/{name}", self.project());
        let Some(instance) = self
            .get_allow_404(&path, &format!("get instance {name}@{zone}"))
            .await?
        else {
            return Ok(None);
        };
        Ok(instance
            .get("status")
            .and_then(Value::as_str)
            .map(str::to_string))
    }

    /// `GET .../aggregated/instances?filter=...`, flattened to
    /// `(zone, instance-json)` pairs across all pages.
    async fn aggregated_instances(&self, filter: &str) -> Result<Vec<(String, Value)>, GceError> {
        let mut out = Vec::new();
        let mut page_token: Option<String> = None;
        loop {
            let mut path = format!(
                "/projects/{}/aggregated/instances?filter={}",
                self.project(),
                crate::queue::gcs::percent_encode(filter)
            );
            if let Some(token) = &page_token {
                path.push_str(&format!(
                    "&pageToken={}",
                    crate::queue::gcs::percent_encode(token)
                ));
            }
            let page = self.get(&path, "aggregatedList instances").await?;
            if let Some(items) = page.get("items").and_then(Value::as_object) {
                for (scope, scoped) in items {
                    let zone = scope.rsplit('/').next().unwrap_or("").to_string();
                    // Zones with no matching instances carry a `warning`
                    // entry instead of an `instances` list — skip those.
                    if let Some(instances) = scoped.get("instances").and_then(Value::as_array) {
                        for instance in instances {
                            out.push((zone.clone(), instance.clone()));
                        }
                    }
                }
            }
            match page.get("nextPageToken").and_then(Value::as_str) {
                Some(token) => page_token = Some(token.to_string()),
                None => break,
            }
        }
        Ok(out)
    }
}

/// Python `"-".join(zone.split("-")[:2])`: "us-central1-b" -> "us-central1".
pub fn region_of_zone(zone: &str) -> String {
    zone.split('-').take(2).collect::<Vec<_>>().join("-")
}

/// Build the REST insert body (Python's `compute_v1.Instance(...)`).
/// Split out pure for tests.
#[allow(clippy::too_many_arguments)]
pub fn instance_body(
    name: &str,
    zone: &str,
    machine_type: &str,
    accel_type: &str,
    boot_disk_gb: i64,
    image: &str,
    image_project: &str,
    startup_script: &str,
    preemptible: bool,
) -> Value {
    let scheduling = if preemptible {
        // Use Spot (the modern provisioning model). The legacy
        // `preemptible` flag is a separate Bool that GCP keeps for
        // back-compat; setting both is redundant but explicit.
        // instanceTerminationAction="DELETE" so a preempted VM is fully
        // removed (disk + instance), not just STOPped. With STOP, every
        // preemption left a zombie TERMINATED instance holding 200GB of
        // regional disk quota — empirically we accumulated 546 of them in
        // 4 days, eating ~109TB and bottlenecking dispatch with
        // DISKS_TOTAL_GB QUOTA_EXCEEDED.
        json!({
            "preemptible": true,
            "provisioningModel": "SPOT",
            "onHostMaintenance": "TERMINATE",
            "instanceTerminationAction": "DELETE",
        })
    } else {
        json!({ "preemptible": false, "onHostMaintenance": "TERMINATE" })
    };
    let guest_accelerators = if accel_type.is_empty() {
        json!([])
    } else {
        json!([{
            "acceleratorType": format!("zones/{zone}/acceleratorTypes/{accel_type}"),
            "acceleratorCount": 1,
        }])
    };
    json!({
        "name": name,
        "machineType": format!("zones/{zone}/machineTypes/{machine_type}"),
        "disks": [{
            "autoDelete": true,
            "boot": true,
            "initializeParams": {
                "diskSizeGb": boot_disk_gb,
                "sourceImage": format!("projects/{image_project}/global/images/{image}"),
            },
        }],
        "networkInterfaces": [{ "accessConfigs": [{ "name": "External NAT" }] }],
        "metadata": { "items": [{ "key": "startup-script", "value": startup_script }] },
        "scheduling": scheduling,
        "guestAccelerators": guest_accelerators,
        // Attach wisent-compute-sa so the instance can write status +
        // heartbeat to GCS, pull HF models with the in-startup token, and
        // fetch from gcloud APIs. Without an SA attached, the metadata
        // service returns 404 for default tokens and the whole startup
        // script crashes before extraction begins.
        "serviceAccounts": [{
            "email": format!("wisent-compute-sa@{}.iam.gserviceaccount.com", config::project()),
            "scopes": [CLOUD_PLATFORM_SCOPE],
        }],
    })
}

/// Resolved-at-first-use provider state (see the module deviation note).
struct GcpState {
    client: GceClient,
    store: JobStorage,
}

/// Python `GCPProvider`.
pub struct GcpProvider {
    state: OnceCell<GcpState>,
}

impl GcpProvider {
    /// Python `GCPProvider()` — lazy in Rust (see the module docs).
    pub fn from_env() -> Self {
        GcpProvider {
            state: OnceCell::new(),
        }
    }

    /// Bind explicit client + storage (tests).

    async fn state(&self) -> Result<&GcpState, ProviderError> {
        self.state
            .get_or_try_init(|| async {
                let client = GceClient::new(config::project()).await?;
                let store = JobStorage::new().await?;
                Ok::<_, ProviderError>(GcpState { client, store })
            })
            .await
    }

    /// Python's `name@zone` ref builder.
    fn reference(name: &str, zone: &str) -> String {
        format!("{name}@{zone}")
    }

    /// Python `name, zone = instance_ref.split("@")` — a ref that does not
    /// split into exactly two parts is a ValueError.
    fn parse_ref(instance_ref: &str) -> Result<(&str, &str), ProviderError> {
        let parts: Vec<&str> = instance_ref.split('@').collect();
        if parts.len() != 2 {
            return Err(ProviderError::Value(format!(
                "invalid instance_ref (expected name@zone): {instance_ref}"
            )));
        }
        Ok((parts[0], parts[1]))
    }

    /// One zone insert attempt after the caller has confirmed any stale
    /// same-name instance is absent. The insert operation is then awaited.
    #[allow(clippy::too_many_arguments)]
    async fn attempt_zone(
        client: &GceClient,
        zone: &str,
        name: &str,
        machine_type: &str,
        accel_type: &str,
        boot_disk_gb: i64,
        image: &str,
        image_project: &str,
        startup_script: &str,
        preemptible: bool,
    ) -> Result<(), GceError> {
        let body = instance_body(
            name,
            zone,
            machine_type,
            accel_type,
            boot_disk_gb,
            image,
            image_project,
            startup_script,
            preemptible,
        );
        let insert_path = format!("/projects/{}/zones/{zone}/instances", client.project());
        let op = client
            .post(&insert_path, &body, &format!("insert {name}@{zone}"))
            .await?;
        let op_name = op.get("name").and_then(Value::as_str).ok_or_else(|| {
            GceError::Api(format!("GCE insert {name}@{zone} -> no operation name"))
        })?;
        client
            .wait_zone_operation(zone, op_name, &format!("insert {name}@{zone}"))
            .await
    }

    /// Python `list_running_instance_refs_with_age`: `(name@zone,
    /// age_in_seconds)` for every wisent-agent VM that is not genuinely
    /// TERMINATED. Used by the dead-agent reaper to cross-reference against
    /// live capacity broadcasts and to apply a boot grace period before
    /// culling.
    ///
    /// The filter intentionally narrows to `<prefix>-agent-*` (not just
    /// `<prefix>-*`): the broader pattern also matches unrelated service
    /// MIG instances in the same project (`wisent-mig-api-*`,
    /// `wisent-mig-inference-*`, `wisent-mig-images-*`) which never
    /// broadcast capacity, so the reaper would mass-delete them every tick.
    pub async fn list_running_instance_refs_with_age(
        &self,
    ) -> Result<Vec<(String, f64)>, ProviderError> {
        let state = self.state().await?;
        let filter = format!("name:{}-agent-*", config::INSTANCE_PREFIX);
        let instances = state.client.aggregated_instances(&filter).await?;
        let now = chrono::Utc::now();
        let mut out = Vec::new();
        for (zone, instance) in instances {
            // Only a genuinely TERMINATED (or absent) instance means the VM
            // is gone. The old `!= "RUNNING"` filter dropped VMs in
            // transient states GCE routinely passes through — PROVISIONING/
            // STAGING on boot and especially REPAIRING/STOPPING/SUSPENDING
            // during host maintenance & live migration (frequent for
            // long-running A100 VMs). A migrating VM briefly leaves this
            // list, the monitor's "cloud agent missing from fleet" path
            // then requeues a perfectly healthy job, and the VM rejoins
            // seconds later. Confirmed live: wisent-agent-a100-1778891822-0
            // was looping normally (agent log through 01:10:36) when the
            // coordinator declared it "VM gone" and requeued Qwen3 724084db
            // at 01:03:37 — a false positive. Treat any non-TERMINATED
            // status as present.
            let status = instance.get("status").and_then(Value::as_str).unwrap_or("");
            if status == "TERMINATED" {
                continue;
            }
            let created = instance
                .get("creationTimestamp")
                .and_then(Value::as_str)
                .unwrap_or("");
            let mut age = 0.0;
            if !created.is_empty() {
                // Python: datetime.fromisoformat(created.replace("Z",
                // "+00:00")); chrono parses RFC3339 "Z" directly.
                if let Ok(ct) = chrono::DateTime::parse_from_rfc3339(created) {
                    age = (now - ct.with_timezone(&chrono::Utc)).num_milliseconds() as f64 / 1000.0;
                }
            }
            let name = instance.get("name").and_then(Value::as_str).unwrap_or("");
            out.push((Self::reference(name, &zone), age));
        }
        Ok(out)
    }

    /// Python `list_running_instance_refs`: `name@zone` refs for all
    /// non-TERMINATED wisent-agent VMs.
    pub async fn list_running_instance_refs(&self) -> Result<Vec<String>, ProviderError> {
        Ok(self
            .list_running_instance_refs_with_age()
            .await?
            .into_iter()
            .map(|(reference, _)| reference)
            .collect())
    }
}

#[async_trait]
impl Provider for GcpProvider {
    #[allow(clippy::too_many_arguments)]
    async fn create_instance(
        &self,
        name: &str,
        machine_type: &str,
        accel_type: &str,
        boot_disk_gb: i64,
        image: &str,
        image_project: &str,
        startup_script: &str,
        preemptible: bool,
    ) -> Result<Option<String>, ProviderError> {
        let state = self.state().await?;
        let client = &state.client;
        let store = &state.store;

        let zones = config::machine_type_zones()
            .get(machine_type)
            .cloned()
            .unwrap_or_else(|| config::zone_rotation().to_vec());
        // Track regions with confirmed QUOTA_EXCEEDED this call. GCP
        // enforces GPU quota at the regional level, so a 403 in one zone
        // means every other zone in the same region will also fail.
        // Without this short-circuit the loop wastes ~10 wall-seconds per
        // zone-retry inside the 60s Cloud Function tick budget — saturated
        // T4 quota in us-central1 alone burned 30+ seconds of every tick
        // today, causing 504s.
        let mut skip_regions: HashSet<String> = HashSet::new();
        for zone in &zones {
            let region = region_of_zone(zone);
            if skip_regions.contains(&region) {
                continue;
            }
            if stockout::zone_recently_stocked_out(store, zone).await? {
                log(&format!(
                    "skip {zone} (recent stockout, TTL {}s)",
                    stockout::STOCKOUT_TTL_S as i64
                ));
                continue;
            }
            // Cross-call quota cache: previous tick's create_instance found
            // this (region, accel) at QUOTA_EXCEEDED. Skip the API call —
            // quota doesn't change within the 60s TTL window.
            if !accel_type.is_empty()
                && stockout::region_recently_quota_exceeded(store, &region, accel_type).await?
            {
                log(&format!(
                    "skip {zone} ({accel_type} quota exhausted in {region}, TTL {}s)",
                    stockout::QUOTA_TTL_S as i64
                ));
                continue;
            }
            // A non-404 delete failure leaves the old VM's existence
            // ambiguous. Abort instead of trying another zone and risking
            // two live VMs writing the same job paths.
            let instance_path = format!(
                "/projects/{}/zones/{zone}/instances/{name}",
                client.project()
            );
            client
                .delete_allow_404(
                    &instance_path,
                    &format!("delete stale instance {name}@{zone}"),
                )
                .await?;
            match Self::attempt_zone(
                client,
                zone,
                name,
                machine_type,
                accel_type,
                boot_disk_gb,
                image,
                image_project,
                startup_script,
                preemptible,
            )
            .await
            {
                Ok(()) => {
                    log(&format!(
                        "Created {} preemptible={}",
                        Self::reference(name, zone),
                        py_bool(preemptible)
                    ));
                    return Ok(Some(Self::reference(name, zone)));
                }
                Err(exc) => {
                    let msg = exc.to_string();
                    if msg.contains("already exists") {
                        return Ok(Some(Self::reference(name, zone)));
                    }
                    // The GCE insert call returns an Operation the moment
                    // the API accepts the request. Waiting for completion
                    // polls; if that wait fails (SSL
                    // UNEXPECTED_EOF_WHILE_READING, RetryError on transient
                    // transport failure, etc.) AFTER the insert was already
                    // accepted server-side, the VM may still come up.
                    // Without a probe here the loop falls through to the
                    // next zone and create_instance spawns a SECOND live VM
                    // with the same name in a different zone. Two VMs
                    // sharing one job_id both write to the same GCS log
                    // path gs://wisent-compute/status/<job>/output/
                    // command_output.log, producing interleaved-writer logs
                    // and double-charged compute. Confirmed live
                    // 2026-05-15: Qwen3 job 724084db had concurrent
                    // subprocesses at step 539 (25s/step) and step 68
                    // (80s/step) in the same log. Probe this zone before
                    // continuing: if the VM actually exists, return its ref
                    // instead of falling through.
                    if let Ok(Some(status)) = client.instance_status(zone, name).await {
                        if matches!(status.as_str(), "RUNNING" | "STAGING" | "PROVISIONING") {
                            log(&format!(
                                "Recovered {} (insert accepted, operation wait raised {}); \
                                 returning existing ref to prevent duplicate in another zone",
                                Self::reference(name, zone),
                                gce_error_kind(&exc)
                            ));
                            return Ok(Some(Self::reference(name, zone)));
                        }
                    }
                    log(&format!("Failed in {zone}: {exc}"));
                    if msg.contains("QUOTA_EXCEEDED") {
                        skip_regions.insert(region.clone());
                        if !accel_type.is_empty() {
                            stockout::mark_region_quota_exceeded(store, &region, accel_type)
                                .await?;
                        }
                    }
                    if msg.contains("ZONE_RESOURCE_POOL_EXHAUSTED") || msg.contains("STOCKOUT") {
                        stockout::mark_zone_stockout(store, zone).await?;
                    }
                    continue;
                }
            }
        }
        Ok(None)
    }

    async fn delete_instance(&self, instance_ref: &str) -> Result<(), ProviderError> {
        let state = self.state().await?;
        let (name, zone) = Self::parse_ref(instance_ref)?;
        let path = format!(
            "/projects/{}/zones/{zone}/instances/{name}",
            state.client.project()
        );
        // Idempotent: already-deleted instance is the desired terminal
        // state. Any other API error propagates so the caller sees it.
        state
            .client
            .delete_allow_404(&path, &format!("delete {instance_ref}"))
            .await?;
        Ok(())
    }

    async fn stop_instance(&self, instance_ref: &str) -> Result<(), ProviderError> {
        let state = self.state().await?;
        let (name, zone) = Self::parse_ref(instance_ref)?;
        let path = format!(
            "/projects/{}/zones/{zone}/instances/{name}/stop",
            state.client.project()
        );
        state
            .client
            .post(&path, &json!({}), &format!("stop {instance_ref}"))
            .await?;
        Ok(())
    }

    async fn start_instance(&self, instance_ref: &str) -> Result<(), ProviderError> {
        let state = self.state().await?;
        let (name, zone) = Self::parse_ref(instance_ref)?;
        let path = format!(
            "/projects/{}/zones/{zone}/instances/{name}/start",
            state.client.project()
        );
        state
            .client
            .post(&path, &json!({}), &format!("start {instance_ref}"))
            .await?;
        Ok(())
    }

    async fn instance_exists(&self, instance_ref: &str) -> Result<bool, ProviderError> {
        let state = self.state().await?;
        let (name, zone) = Self::parse_ref(instance_ref)?;
        let Some(status) = state.client.instance_status(zone, name).await? else {
            return Ok(false);
        };
        Ok(matches!(
            status.as_str(),
            "RUNNING" | "STAGING" | "PROVISIONING"
        ))
    }

    async fn instance_lifecycle_state(
        &self,
        instance_ref: &str,
    ) -> Result<Option<String>, ProviderError> {
        let state = self.state().await?;
        let (name, zone) = Self::parse_ref(instance_ref)?;
        Ok(state.client.instance_status(zone, name).await?)
    }

    /// Trait override delegating to the inherent method (kept for direct
    /// GcpProvider callers) so `&dyn Provider` consumers — the dead-agent
    /// reaper — can reach it.
    async fn list_running_instance_refs_with_age(
        &self,
    ) -> Result<Vec<(String, f64)>, ProviderError> {
        GcpProvider::list_running_instance_refs_with_age(self).await
    }

    async fn list_running_instances(&self) -> Result<BTreeMap<String, i64>, ProviderError> {
        let state = self.state().await?;
        let filter = format!("name:{}-*", config::INSTANCE_PREFIX);
        let instances = state.client.aggregated_instances(&filter).await?;
        let mut counts: BTreeMap<String, i64> = BTreeMap::new();
        for (_zone, instance) in instances {
            let status = instance.get("status").and_then(Value::as_str).unwrap_or("");
            if !matches!(status, "RUNNING" | "STAGING" | "PROVISIONING") {
                continue;
            }
            if let Some(accelerators) = instance.get("guestAccelerators").and_then(Value::as_array)
            {
                for accel in accelerators {
                    let atype = accel
                        .get("acceleratorType")
                        .and_then(Value::as_str)
                        .and_then(|t| t.rsplit('/').next())
                        .unwrap_or("");
                    if atype.is_empty() {
                        continue;
                    }
                    let count = accel
                        .get("acceleratorCount")
                        .and_then(Value::as_i64)
                        .unwrap_or(0);
                    *counts.entry(atype.to_string()).or_insert(0) += count;
                }
            }
        }
        Ok(counts)
    }
}

/// The Python `type(exc).__name__` slot in the "Recovered ..." log line.
fn gce_error_kind(exc: &GceError) -> &'static str {
    match exc {
        GceError::Auth(_) => "DefaultCredentialsError",
        GceError::Http(_) => "RetryError",
        GceError::Api(_) => "GoogleAPICallError",
    }
}

