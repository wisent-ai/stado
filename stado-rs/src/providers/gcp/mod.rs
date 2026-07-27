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

/// The baked agent image family (Python `baked_family`). The
/// bake_agent_image.sh script publishes images into family 'wisent-agent'
/// in the local project; when present, every dispatched VM uses the baked
/// image instead of the legacy deeplearning-platform-release base.
const BAKED_IMAGE_FAMILY: &str = "wisent-agent";

/// Python `_log`.
fn log(msg: &str) {
    eprintln!("[gcp] {msg}");
}

/// Python f-string rendering of a bool.
fn py_bool(value: bool) -> &'static str {
    if value { "True" } else { "False" }
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
        let auth = gcp_auth::provider().await.map_err(|err| GceError::Auth(err.to_string()))?;
        Ok(Self::assemble(project, COMPUTE_API_BASE, Some(auth), Duration::from_secs(1)))
    }

    /// Bind to an explicit base URL without credentials (loopback mocks in
    /// tests) and with a near-zero LRO poll interval.
    #[cfg(test)]
    pub(crate) fn for_test(base_url: &str, project: &str) -> Self {
        Self::assemble(project, base_url, None, Duration::from_millis(1))
    }

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
        let Some(auth) = &self.inner.auth else { return Ok(None) };
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
        let path =
            format!("/projects/{}/zones/{zone}/operations/{operation}", self.project());
        loop {
            let op = self.get(&path, &format!("get operation {operation}")).await?;
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
        let Some(instance) =
            self.get_allow_404(&path, &format!("get instance {name}@{zone}")).await?
        else {
            return Ok(None);
        };
        Ok(instance.get("status").and_then(Value::as_str).map(str::to_string))
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
                path.push_str(&format!("&pageToken={}", crate::queue::gcs::percent_encode(token)));
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
        GcpProvider { state: OnceCell::new() }
    }

    /// Bind explicit client + storage (tests).
    #[cfg(test)]
    pub(crate) fn with_client_and_store(client: GceClient, store: JobStorage) -> Self {
        let state = OnceCell::new();
        let _ = state.set(GcpState { client, store });
        GcpProvider { state }
    }

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

    /// One zone attempt of the Python create loop: pre-delete any existing
    /// (terminated) instance with the same name — NotFound is the desired
    /// terminal state; anything else propagates — then insert and wait for
    /// the zone operation.
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
        let instance_path =
            format!("/projects/{}/zones/{zone}/instances/{name}", client.project());
        client
            .delete_allow_404(&instance_path, &format!("delete stale instance {name}@{zone}"))
            .await?;
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
        let op = client.post(&insert_path, &body, &format!("insert {name}@{zone}")).await?;
        let op_name = op
            .get("name")
            .and_then(Value::as_str)
            .ok_or_else(|| GceError::Api(format!("GCE insert {name}@{zone} -> no operation name")))?;
        client.wait_zone_operation(zone, op_name, &format!("insert {name}@{zone}")).await
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
            let created = instance.get("creationTimestamp").and_then(Value::as_str).unwrap_or("");
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

        // Override per-job stored image if a baked agent image family
        // exists. When present, every dispatched VM uses the baked image
        // (which already has wisent-compute + transformers + datasets
        // pre-installed) instead of the legacy deeplearning-platform-
        // release base, dropping boot time from ~5-10 install-rotations to
        // ~30 install-secs. No baked image family published yet (404) ->
        // use the per-job image argument unchanged. Any other error
        // propagates.
        let mut image = image.to_string();
        let mut image_project = image_project.to_string();
        let family_path =
            format!("/projects/{}/global/images/family/{BAKED_IMAGE_FAMILY}", client.project());
        if let Some(latest) =
            client.get_allow_404(&family_path, "get image family wisent-agent").await?
        {
            if let Some(latest_name) =
                latest.get("name").and_then(Value::as_str).filter(|s| !s.is_empty())
            {
                image = latest_name.to_string();
                image_project = client.project().to_string();
            }
        }

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
            match Self::attempt_zone(
                client,
                zone,
                name,
                machine_type,
                accel_type,
                boot_disk_gb,
                &image,
                &image_project,
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
        let path =
            format!("/projects/{}/zones/{zone}/instances/{name}", state.client.project());
        // Idempotent: already-deleted instance is the desired terminal
        // state. Any other API error propagates so the caller sees it.
        state.client.delete_allow_404(&path, &format!("delete {instance_ref}")).await?;
        Ok(())
    }

    async fn instance_exists(&self, instance_ref: &str) -> Result<bool, ProviderError> {
        let state = self.state().await?;
        let (name, zone) = Self::parse_ref(instance_ref)?;
        let Some(status) = state.client.instance_status(zone, name).await? else {
            return Ok(false);
        };
        Ok(matches!(status.as_str(), "RUNNING" | "STAGING" | "PROVISIONING"))
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
            if let Some(accelerators) =
                instance.get("guestAccelerators").and_then(Value::as_array)
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
                    let count =
                        accel.get("acceleratorCount").and_then(Value::as_i64).unwrap_or(0);
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::queue::local_file::LocalBackend;
    use crate::testutil::{http_response, mock_http, MockHttp};

    fn store() -> (tempfile::TempDir, JobStorage) {
        let dir = tempfile::tempdir().unwrap();
        let backend = LocalBackend::new(dir.path().to_str().unwrap()).unwrap();
        (dir, JobStorage::with_backend(Arc::new(backend), "local"))
    }

    fn provider_for(server: &MockHttp, store: &JobStorage) -> GcpProvider {
        GcpProvider::with_client_and_store(
            GceClient::for_test(&server.base_url, "test-project"),
            store.clone(),
        )
    }

    const DONE: &str = r#"{"name": "operation-1", "status": "DONE"}"#;
    const PENDING: &str = r#"{"name": "operation-1", "status": "PENDING"}"#;

    fn request_bodies(server: &MockHttp) -> Vec<String> {
        server.requests.lock().unwrap().clone()
    }

    #[test]
    fn instance_body_spot_and_on_demand_shapes() {
        let spot = instance_body(
            "vm1",
            "us-central1-b",
            "n1-standard-4",
            "nvidia-tesla-t4",
            200,
            "base-image",
            "deeplearning-platform-release",
            "#!/bin/bash\necho hi",
            true,
        );
        assert_eq!(spot["machineType"], json!("zones/us-central1-b/machineTypes/n1-standard-4"));
        assert_eq!(
            spot["disks"][0]["initializeParams"]["sourceImage"],
            json!("projects/deeplearning-platform-release/global/images/base-image")
        );
        assert_eq!(spot["disks"][0]["initializeParams"]["diskSizeGb"], json!(200));
        assert_eq!(
            spot["scheduling"],
            json!({
                "preemptible": true,
                "provisioningModel": "SPOT",
                "onHostMaintenance": "TERMINATE",
                "instanceTerminationAction": "DELETE",
            })
        );
        assert_eq!(
            spot["guestAccelerators"][0]["acceleratorType"],
            json!("zones/us-central1-b/acceleratorTypes/nvidia-tesla-t4")
        );
        assert_eq!(
            spot["serviceAccounts"][0]["email"],
            json!(format!(
                "wisent-compute-sa@{}.iam.gserviceaccount.com",
                config::project()
            ))
        );
        assert_eq!(spot["metadata"]["items"][0]["key"], json!("startup-script"));

        // On-demand: no Spot fields; empty accel -> no guest accelerators.
        let on_demand = instance_body(
            "vm1", "us-central1-b", "n1-standard-4", "", 200, "img", "proj", "", false,
        );
        assert_eq!(
            on_demand["scheduling"],
            json!({ "preemptible": false, "onHostMaintenance": "TERMINATE" })
        );
        assert_eq!(on_demand["guestAccelerators"], json!([]));
    }

    #[test]
    fn region_of_zone_drops_the_zone_suffix() {
        assert_eq!(region_of_zone("us-central1-b"), "us-central1");
        assert_eq!(region_of_zone("europe-west4-a"), "europe-west4");
    }

    #[tokio::test]
    async fn create_instance_happy_path_spot() {
        let _guard = stockout::test_lock().await;
        let (_dir, store) = store();
        let server = mock_http(vec![
            http_response(404, "Not Found", r#"{"error": {"code": 404, "message": "not found"}}"#),
            http_response(404, "Not Found", r#"{"error": {"code": 404, "message": "not found"}}"#),
            http_response(200, "OK", PENDING),
            http_response(200, "OK", DONE),
        ])
        .await;
        let provider = provider_for(&server, &store);
        let result = provider
            .create_instance(
                "vm1",
                "n1-standard-4",
                "nvidia-tesla-t4",
                200,
                "base-image",
                "deeplearning-platform-release",
                "echo hi",
                true,
            )
            .await
            .unwrap();
        assert_eq!(result.as_deref(), Some("vm1@us-central1-b"));

        let requests = request_bodies(&server);
        assert_eq!(requests.len(), 4, "{requests:?}");
        // The mock server replaces the whole API base URL, so paths start
        // at /projects/... (no /compute/v1 prefix).
        assert!(requests[0].starts_with(
            "GET /projects/test-project/global/images/family/wisent-agent "
        ), "{}", requests[0]);
        assert!(requests[1].starts_with(
            "DELETE /projects/test-project/zones/us-central1-b/instances/vm1 "
        ), "{}", requests[1]);
        assert!(requests[2].starts_with(
            "POST /projects/test-project/zones/us-central1-b/instances "
        ), "{}", requests[2]);
        assert!(requests[2].contains(r#""provisioningModel":"SPOT""#), "{}", requests[2]);
        assert!(requests[2].contains(r#""startup-script","value":"echo hi""#), "{}", requests[2]);
        assert!(requests[3].starts_with(
            "GET /projects/test-project/zones/us-central1-b/operations/operation-1 "
        ), "{}", requests[3]);
        server.stop();
    }

    #[tokio::test]
    async fn create_instance_prefers_baked_image_family() {
        let _guard = stockout::test_lock().await;
        let (_dir, store) = store();
        let server = mock_http(vec![
            http_response(200, "OK", r#"{"name": "wisent-agent-20260501", "status": "READY"}"#),
            http_response(404, "Not Found", r#"{"error": {"code": 404}}"#),
            http_response(200, "OK", PENDING),
            http_response(200, "OK", DONE),
        ])
        .await;
        let provider = provider_for(&server, &store);
        let result = provider
            .create_instance(
                "vm1", "n1-standard-4", "", 200, "base-image", "deeplearning-platform-release",
                "echo hi", false,
            )
            .await
            .unwrap();
        assert_eq!(result.as_deref(), Some("vm1@us-central1-b"));
        let requests = request_bodies(&server);
        assert!(
            requests[2].contains(r#""sourceImage":"projects/test-project/global/images/wisent-agent-20260501""#),
            "{}",
            requests[2]
        );
        server.stop();
    }

    #[tokio::test]
    async fn quota_exceeded_skips_the_rest_of_the_region() {
        let _guard = stockout::test_lock().await;
        let (_dir, store) = store();
        // us-central1-b fails with a QUOTA_EXCEEDED LRO error; every other
        // us-central1 zone must be skipped without an API call, and the
        // loop resumes at europe-west4-a.
        let server = mock_http(vec![
            http_response(404, "Not Found", r#"{"error": {"code": 404}}"#),
            http_response(404, "Not Found", r#"{"error": {"code": 404}}"#),
            http_response(200, "OK", PENDING),
            http_response(
                200,
                "OK",
                r#"{"name": "operation-1", "status": "DONE", "error": {"errors": [{"code": "QUOTA_EXCEEDED", "message": "Quota 'NVIDIA_T4_GPUS' exceeded. Limit: 8.0 in region us-central1."}]}}"#,
            ),
            // The duplicate-VM probe after the failed operation wait.
            http_response(404, "Not Found", r#"{"error": {"code": 404}}"#),
            http_response(404, "Not Found", r#"{"error": {"code": 404}}"#),
            http_response(200, "OK", r#"{"name": "operation-2", "status": "RUNNING"}"#),
            http_response(200, "OK", r#"{"name": "operation-2", "status": "DONE"}"#),
        ])
        .await;
        let provider = provider_for(&server, &store);
        let result = provider
            .create_instance(
                "vm1", "n1-standard-4", "nvidia-tesla-t4", 200, "img", "proj", "echo hi", false,
            )
            .await
            .unwrap();
        assert_eq!(result.as_deref(), Some("vm1@europe-west4-a"));
        let requests = request_bodies(&server);
        assert_eq!(requests.len(), 8, "{requests:?}");
        assert!(requests[5].starts_with(
            "DELETE /projects/test-project/zones/europe-west4-a/instances/vm1 "
        ), "{}", requests[5]);
        assert!(!requests.iter().any(|r| r.contains("us-central1-a")), "{requests:?}");
        assert!(!requests.iter().any(|r| r.contains("us-central1-c")), "{requests:?}");
        // The cross-call quota cache was marked for (region, accel).
        let blob = store
            .download_text(stockout::QUOTA_BLOB)
            .await
            .unwrap()
            .expect("quota blob written");
        assert!(blob.contains("us-central1:nvidia-tesla-t4"), "{blob}");
        server.stop();
    }

    #[tokio::test]
    async fn stockout_cache_skips_zone_and_marks_new_stockouts() {
        let _guard = stockout::test_lock().await;
        let (_dir, store) = store();
        // Pre-mark us-central1-b as stocked out; the loop's first API call
        // must target us-central1-a. That zone then stockouts live and the
        // cache picks it up.
        stockout::mark_zone_stockout(&store, "us-central1-b").await.unwrap();
        let server = mock_http(vec![
            http_response(404, "Not Found", r#"{"error": {"code": 404}}"#),
            http_response(404, "Not Found", r#"{"error": {"code": 404}}"#),
            http_response(200, "OK", PENDING),
            http_response(
                200,
                "OK",
                r#"{"name": "operation-1", "status": "DONE", "error": {"errors": [{"code": "ZONE_RESOURCE_POOL_EXHAUSTED", "message": "The zone does not have enough resources available to fulfill the request."}]}}"#,
            ),
            // The duplicate-VM probe after the failed operation wait.
            http_response(404, "Not Found", r#"{"error": {"code": 404}}"#),
            http_response(404, "Not Found", r#"{"error": {"code": 404}}"#),
            http_response(200, "OK", r#"{"name": "operation-2", "status": "PENDING"}"#),
            http_response(200, "OK", r#"{"name": "operation-2", "status": "DONE"}"#),
        ])
        .await;
        let provider = provider_for(&server, &store);
        let result = provider
            .create_instance(
                "vm1", "n1-standard-4", "nvidia-tesla-t4", 200, "img", "proj", "echo hi", false,
            )
            .await
            .unwrap();
        assert_eq!(result.as_deref(), Some("vm1@us-central1-c"));
        let requests = request_bodies(&server);
        assert_eq!(requests.len(), 8, "{requests:?}");
        // First instance call went to us-central1-a (b was cache-skipped).
        assert!(requests[1].contains("zones/us-central1-a/instances/vm1"), "{}", requests[1]);
        // The live stockout of us-central1-a was marked.
        assert!(stockout::zone_recently_stocked_out(&store, "us-central1-a").await.unwrap());
        assert!(stockout::zone_recently_stocked_out(&store, "us-central1-b").await.unwrap());
        server.stop();
    }

    #[tokio::test]
    async fn already_exists_returns_the_ref() {
        let _guard = stockout::test_lock().await;
        let (_dir, store) = store();
        let server = mock_http(vec![
            http_response(404, "Not Found", r#"{"error": {"code": 404}}"#),
            http_response(404, "Not Found", r#"{"error": {"code": 404}}"#),
            http_response(
                409,
                "Conflict",
                r#"{"error": {"code": 409, "message": "The resource 'projects/test-project/zones/us-central1-b/instances/vm1' already exists", "errors": [{"message": "The resource 'projects/test-project/zones/us-central1-b/instances/vm1' already exists", "domain": "global", "reason": "alreadyExists"}]}}"#,
            ),
        ])
        .await;
        let provider = provider_for(&server, &store);
        let result = provider
            .create_instance(
                "vm1", "n1-standard-4", "nvidia-tesla-t4", 200, "img", "proj", "echo hi", false,
            )
            .await
            .unwrap();
        assert_eq!(result.as_deref(), Some("vm1@us-central1-b"));
        server.stop();
    }

    #[tokio::test]
    async fn poll_failure_probes_and_recovers_live_instance() {
        let _guard = stockout::test_lock().await;
        let (_dir, store) = store();
        // The operation poll fails AFTER the insert was accepted; the probe
        // finds the VM STAGING and the ref is returned instead of spawning
        // a duplicate in another zone.
        let server = mock_http(vec![
            http_response(404, "Not Found", r#"{"error": {"code": 404}}"#),
            http_response(404, "Not Found", r#"{"error": {"code": 404}}"#),
            http_response(200, "OK", PENDING),
            http_response(
                500,
                "Internal Server Error",
                r#"{"error": {"code": 500, "message": "SSL UNEXPECTED_EOF_WHILE_READING", "errors": [{"reason": "backendError", "message": "SSL UNEXPECTED_EOF_WHILE_READING"}]}}"#,
            ),
            http_response(200, "OK", r#"{"name": "vm1", "status": "STAGING"}"#),
        ])
        .await;
        let provider = provider_for(&server, &store);
        let result = provider
            .create_instance(
                "vm1", "n1-standard-4", "nvidia-tesla-t4", 200, "img", "proj", "echo hi", false,
            )
            .await
            .unwrap();
        assert_eq!(result.as_deref(), Some("vm1@us-central1-b"));
        assert_eq!(request_bodies(&server).len(), 5);
        server.stop();
    }

    #[tokio::test]
    async fn probe_miss_falls_through_to_the_next_zone() {
        let _guard = stockout::test_lock().await;
        let (_dir, store) = store();
        let server = mock_http(vec![
            http_response(404, "Not Found", r#"{"error": {"code": 404}}"#),
            http_response(404, "Not Found", r#"{"error": {"code": 404}}"#),
            http_response(200, "OK", PENDING),
            http_response(
                500,
                "Internal Server Error",
                r#"{"error": {"code": 500, "message": "transient", "errors": [{"reason": "backendError", "message": "transient"}]}}"#,
            ),
            http_response(404, "Not Found", r#"{"error": {"code": 404}}"#),
            http_response(404, "Not Found", r#"{"error": {"code": 404}}"#),
            http_response(200, "OK", r#"{"name": "operation-2", "status": "PENDING"}"#),
            http_response(200, "OK", r#"{"name": "operation-2", "status": "DONE"}"#),
        ])
        .await;
        let provider = provider_for(&server, &store);
        let result = provider
            .create_instance(
                "vm1", "n1-standard-4", "nvidia-tesla-t4", 200, "img", "proj", "echo hi", false,
            )
            .await
            .unwrap();
        assert_eq!(result.as_deref(), Some("vm1@us-central1-a"));
        server.stop();
    }

    #[tokio::test]
    async fn all_zones_unavailable_returns_none() {
        let _guard = stockout::test_lock().await;
        let (_dir, store) = store();
        // Stockout-cache every rotation zone: no zone is even attempted, so
        // the only HTTP call is the image-family lookup. Capacity
        // exhaustion is Ok(None), not an error.
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs_f64();
        let map: BTreeMap<String, f64> =
            config::zone_rotation().iter().map(|z| (z.clone(), now)).collect();
        store
            .upload_text(stockout::STOCKOUT_BLOB, &serde_json::to_string(&map).unwrap())
            .await
            .unwrap();
        let server = mock_http(vec![http_response(
            404,
            "Not Found",
            r#"{"error": {"code": 404}}"#,
        )])
        .await;
        let provider = provider_for(&server, &store);
        let result = provider
            .create_instance("vm1", "n1-standard-4", "", 200, "img", "proj", "echo hi", false)
            .await
            .unwrap();
        assert_eq!(result, None);
        assert_eq!(request_bodies(&server).len(), 1);
        server.stop();
    }

    #[tokio::test]
    async fn family_lookup_failure_is_an_error_not_capacity_exhaustion() {
        let _guard = stockout::test_lock().await;
        let (_dir, store) = store();
        let server = mock_http(vec![http_response(
            500,
            "Internal Server Error",
            r#"{"error": {"code": 500, "message": "backend exploded", "errors": [{"reason": "backendError", "message": "backend exploded"}]}}"#,
        )])
        .await;
        let provider = provider_for(&server, &store);
        let err = provider
            .create_instance("vm1", "n1-standard-4", "", 200, "img", "proj", "echo hi", false)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("HTTP 500"), "{err}");
        server.stop();
    }

    #[tokio::test]
    async fn delete_instance_404_is_success_other_errors_propagate() {
        let (_dir, store) = store();
        let server = mock_http(vec![
            http_response(404, "Not Found", r#"{"error": {"code": 404}}"#),
            http_response(200, "OK", r#"{"name": "operation-9", "status": "PENDING"}"#),
            http_response(
                403,
                "Forbidden",
                r#"{"error": {"code": 403, "message": "Forbidden", "errors": [{"reason": "forbidden", "message": "Forbidden"}]}}"#,
            ),
        ])
        .await;
        let provider = provider_for(&server, &store);
        provider.delete_instance("vm1@us-central1-b").await.unwrap();
        provider.delete_instance("vm1@us-central1-b").await.unwrap();
        let err = provider.delete_instance("vm1@us-central1-b").await.unwrap_err();
        assert!(err.to_string().contains("HTTP 403"), "{err}");
        server.stop();

        // Malformed refs are a ValueError, like Python's unpack.
        let err = provider.delete_instance("no-zone").await.unwrap_err();
        assert!(matches!(err, ProviderError::Value(_)), "{err:?}");
    }

    #[tokio::test]
    async fn instance_exists_and_lifecycle_state() {
        let (_dir, store) = store();
        let server = mock_http(vec![
            http_response(404, "Not Found", r#"{"error": {"code": 404}}"#),
            http_response(200, "OK", r#"{"name": "vm1", "status": "RUNNING"}"#),
            http_response(200, "OK", r#"{"name": "vm1", "status": "TERMINATED"}"#),
            http_response(200, "OK", r#"{"name": "vm1", "status": "STOPPING"}"#),
            http_response(404, "Not Found", r#"{"error": {"code": 404}}"#),
        ])
        .await;
        let provider = provider_for(&server, &store);
        assert!(!provider.instance_exists("vm1@us-central1-b").await.unwrap());
        assert!(provider.instance_exists("vm1@us-central1-b").await.unwrap());
        assert!(!provider.instance_exists("vm1@us-central1-b").await.unwrap());
        assert_eq!(
            provider.instance_lifecycle_state("vm1@us-central1-b").await.unwrap().as_deref(),
            Some("STOPPING")
        );
        assert_eq!(provider.instance_lifecycle_state("vm1@us-central1-b").await.unwrap(), None);
        server.stop();
    }

    #[tokio::test]
    async fn list_running_instances_counts_accelerators_by_type() {
        let (_dir, store) = store();
        let page1 = r#"{
            "items": {
                "zones/us-central1-b": {"instances": [
                    {"name": "wisent-agent-t4-1", "status": "RUNNING", "guestAccelerators": [
                        {"acceleratorType": "projects/p/zones/us-central1-b/acceleratorTypes/nvidia-tesla-t4", "acceleratorCount": 1}]},
                    {"name": "wisent-agent-t4-2", "status": "PROVISIONING", "guestAccelerators": [
                        {"acceleratorType": "projects/p/zones/us-central1-b/acceleratorTypes/nvidia-tesla-t4", "acceleratorCount": 1}]},
                    {"name": "wisent-old", "status": "TERMINATED", "guestAccelerators": [
                        {"acceleratorType": "projects/p/zones/us-central1-b/acceleratorTypes/nvidia-tesla-t4", "acceleratorCount": 1}]}
                ]},
                "zones/europe-west4-a": {"warning": {"code": "NO_RESULTS_ON_PAGE", "message": "No results for the request"}}
            },
            "nextPageToken": "page-2"
        }"#;
        let page2 = r#"{
            "items": {
                "zones/us-east1-c": {"instances": [
                    {"name": "wisent-agent-l4", "status": "STAGING", "guestAccelerators": [
                        {"acceleratorType": "projects/p/zones/us-east1-c/acceleratorTypes/nvidia-l4", "acceleratorCount": 1}]}
                ]}
            }
        }"#;
        let server =
            mock_http(vec![http_response(200, "OK", page1), http_response(200, "OK", page2)]).await;
        let provider = provider_for(&server, &store);
        let counts = provider.list_running_instances().await.unwrap();
        assert_eq!(
            counts,
            BTreeMap::from([("nvidia-l4".to_string(), 1), ("nvidia-tesla-t4".to_string(), 2)])
        );
        let requests = request_bodies(&server);
        // The exact Python filter: name:{INSTANCE_PREFIX}-*.
        assert!(
            requests[0].replace("%3A", ":").replace("%2A", "*").contains("filter=name:wisent-*"),
            "{}",
            requests[0]
        );
        assert!(requests[1].contains("pageToken=page-2"), "{}", requests[1]);
        server.stop();
    }

    #[tokio::test]
    async fn list_running_instance_refs_with_age_filters_terminated() {
        let (_dir, store) = store();
        let body = r#"{
            "items": {
                "zones/us-central1-b": {"instances": [
                    {"name": "wisent-agent-1", "status": "RUNNING",
                     "creationTimestamp": "2020-01-01T00:00:00.000+00:00"},
                    {"name": "wisent-agent-2", "status": "REPAIRING"},
                    {"name": "wisent-agent-3", "status": "TERMINATED"}
                ]}
            }
        }"#;
        let server = mock_http(vec![http_response(200, "OK", body)]).await;
        let provider = provider_for(&server, &store);
        let refs = provider.list_running_instance_refs_with_age().await.unwrap();
        assert_eq!(refs.len(), 2, "{refs:?}");
        assert_eq!(refs[0].0, "wisent-agent-1@us-central1-b");
        assert!(refs[0].1 > 0.0, "age in seconds since boot: {:?}", refs[0]);
        // No creationTimestamp -> age 0.0 (Python parity).
        assert_eq!(refs[1], ("wisent-agent-2@us-central1-b".to_string(), 0.0));
        let requests = request_bodies(&server);
        // The narrow agent-only filter (see the method docs).
        assert!(
            requests[0]
                .replace("%3A", ":")
                .replace("%2A", "*")
                .contains("filter=name:wisent-agent-*"),
            "{}",
            requests[0]
        );
        server.stop();
    }
}
