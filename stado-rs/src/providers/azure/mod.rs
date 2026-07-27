//! Azure provider: VM lifecycle over ARM REST, mirrors providers/gcp.
//!
//! Port of `stado/providers/azure.py`. Python uses azure-identity +
//! azure-mgmt-compute/azure-mgmt-network; this port talks to the ARM REST
//! API (`https://management.azure.com`) directly with reqwest — no Azure
//! SDK crates. NIC + VM create across AZURE_LOCATIONS, falling through
//! quota/capacity errors per region. Pre-provisioned vnet/NSG (named with
//! `-{location}` suffix, one per region in a shared RG) attach to the NIC;
//! the provider does not create networking. instance_ref is
//! `"name@location"`.
//!
//! Authentication is shared with the Azure Blob queue backend through
//! [`crate::azure_token`]: an Azure managed identity is preferred, then the
//! `stado-azure` service-principal item is read from Skarbiec. This module
//! requests the ARM audience (`https://management.azure.com`).
//!
//! On an agent VM IMDS resolves only when the VM carries a managed identity.
//! Agent VMs are therefore created with the pre-provisioned user-assigned
//! identity named by
//! [`crate::config::azure_vm_identity_id`] (`AZURE_VM_IDENTITY_ID`), whose
//! resource id [`vm_body`] renders into the ARM `identity` block. The
//! operator grants that single identity, once:
//!
//! - `Storage Blob Data Contributor` on the queue storage account. That is
//!   a data-plane role; `Contributor` is management-plane only and does NOT
//!   authorize blob reads or writes.
//! - Permission to delete VMs in the resource group, so an idle agent can
//!   ARM-DELETE itself.
//!
//! Without it the agent can neither reach the blob queue (it never sees a
//! job, so the fleet is inert) nor self-delete (the VM bills until someone
//! notices). User-assigned rather than system-assigned is deliberate: a
//! system-assigned principal is minted per VM, so it would need its own
//! role assignment at create time and would leave orphans behind at
//! self-delete. Creating the identity and granting it those roles is an
//! operator provisioning step — this provider hands out no role
//! assignments, just as it creates no networking.
//!
//! Long-running operations are polled via the Azure-AsyncOperation header
//! (falling back to Location) until terminal — the equivalent of the Python
//! SDK's `op.result()`.
//!
//! Deviation: Python's `AzureProvider()` constructor eagerly raises
//! RuntimeError when AZURE_SUBSCRIPTION_ID is empty. Here
//! [`AzureProvider::from_env`] is lazy (same pattern as
//! [`super::gcp::GcpProvider`]): the check fires on the first API call so
//! `get_provider("azure")` stays a cheap, sync factory.

pub mod network;

use std::collections::{BTreeMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use base64::Engine as _;
use serde_json::{json, Value};
use tokio::sync::OnceCell;

use crate::catalog::AZURE_VM_TO_ACCEL;
use crate::config;

use super::{Provider, ProviderError};

/// ARM REST base.
pub const ARM_API_BASE: &str = "https://management.azure.com";
/// Compute RP API version for VM resource paths. Crate-visible so the
/// agent's self-delete ([`crate::providers::local::azure_self`]) targets
/// the same VM contract this provider creates against.
pub(crate) const COMPUTE_API_VERSION: &str = "2023-09-01";
const NETWORK_API_VERSION: &str = "2023-09-01";
/// OAuth scope for the client-credentials token request.
const ARM_SCOPE: &str = "https://management.azure.com/.default";
/// Resource for IMDS / az-CLI token requests.
const ARM_RESOURCE: &str = "https://management.azure.com";

/// Python `_log`.
fn log(msg: &str) {
    eprintln!("[azure] {msg}");
}

/// Python f-string rendering of a bool.
fn py_bool(value: bool) -> &'static str {
    if value {
        "True"
    } else {
        "False"
    }
}

/// Azure auth/transport/API error. The `Api` message embeds the ARM
/// `error.code` + `error.message` so the Python substring classification
/// ("QuotaExceeded", "OperationNotAllowed", "SkuNotAvailable", "already
/// exists") works on `error.to_string()`.
#[derive(Debug, thiserror::Error)]
pub enum AzureError {
    /// Token acquisition failed (or every chain source unavailable).
    #[error("no Azure credentials: {0}")]
    Auth(String),
    /// Transport failure.
    #[error(transparent)]
    Http(#[from] reqwest::Error),
    /// Non-2xx ARM response or failed LRO; message carries code + text.
    #[error("{0}")]
    Api(String),
}

// --- Managed-identity / Skarbiec token source ---

/// Fresh bearer token for ARM, from the shared chain's per-scope cache.
async fn bearer_token(http: &reqwest::Client) -> Result<String, AzureError> {
    crate::azure_token::bearer_token(http, ARM_SCOPE, ARM_RESOURCE)
        .await
        .map_err(|err| match err {
            crate::azure_token::TokenError::Auth(msg) => AzureError::Auth(msg),
            crate::azure_token::TokenError::Http(err) => AzureError::Http(err),
        })
}

// --- ARM REST client ---

/// Bearer-authenticated ARM REST client. Cheap to clone.
#[derive(Clone)]
pub struct ArmClient {
    inner: Arc<ArmInner>,
}

struct ArmInner {
    http: reqwest::Client,
    subscription: String,
    base_url: String,
    /// True in prod (token chain attached); false on loopback test mocks.
    auth: bool,
    /// Delay between LRO polls (near-zero in tests).
    poll_interval: Duration,
}

impl ArmClient {
    /// Bind to the public ARM API; the token chain resolves on the first
    /// request.
    pub fn new(subscription: &str) -> Self {
        Self::assemble(subscription, ARM_API_BASE, true, Duration::from_secs(2))
    }

    /// Bind to an explicit base URL without auth (loopback mocks in
    /// tests) and with a near-zero LRO poll interval.
    #[cfg(test)]
    pub(crate) fn for_test(base_url: &str, subscription: &str) -> Self {
        Self::assemble(subscription, base_url, false, Duration::from_millis(1))
    }

    fn assemble(subscription: &str, base_url: &str, auth: bool, poll_interval: Duration) -> Self {
        ArmClient {
            inner: Arc::new(ArmInner {
                http: reqwest::Client::new(),
                subscription: subscription.to_string(),
                base_url: base_url.trim_end_matches('/').to_string(),
                auth,
                poll_interval,
            }),
        }
    }

    /// The subscription this client is bound to (Python
    /// `self.subscription`).
    pub fn subscription(&self) -> &str {
        &self.inner.subscription
    }

    /// Send one request; the raw response is returned unchecked so
    /// callers can apply their own status handling (404 allowances, LRO
    /// headers). `url` may be a path under the API base or an absolute
    /// URL (LRO poll targets).
    async fn send(
        &self,
        method: reqwest::Method,
        url: &str,
        body: Option<&Value>,
    ) -> Result<reqwest::Response, AzureError> {
        let full = if url.starts_with("http") {
            url.to_string()
        } else {
            format!("{}{url}", self.inner.base_url)
        };
        let mut request = self
            .inner
            .http
            .request(method, full)
            .header(reqwest::header::ACCEPT, "application/json");
        if self.inner.auth {
            let token = bearer_token(&self.inner.http).await?;
            request = request.header(reqwest::header::AUTHORIZATION, format!("Bearer {token}"));
        }
        if let Some(body) = body {
            request = request
                .header(reqwest::header::CONTENT_TYPE, "application/json")
                .body(serde_json::to_string(body).unwrap_or_else(|_| "{}".into()));
        }
        Ok(request.send().await?)
    }

    /// Lift a non-2xx response into [`AzureError::Api`], embedding the
    /// ARM `error.code` + `error.message` so Python's substring
    /// classification keeps working.
    async fn api_error(response: reqwest::Response, desc: &str) -> AzureError {
        let status = response.status().as_u16();
        let text = response.text().await.unwrap_or_default();
        let parsed = serde_json::from_str::<Value>(&text).unwrap_or(Value::Null);
        let error = parsed.get("error").cloned().unwrap_or(Value::Null);
        let code = error.get("code").and_then(Value::as_str).unwrap_or("");
        let message = error.get("message").and_then(Value::as_str).unwrap_or("");
        let detail = if code.is_empty() && message.is_empty() {
            text.chars().take(280).collect::<String>()
        } else {
            format!("{code} {message}").trim().to_string()
        };
        AzureError::Api(format!("Azure {desc} -> HTTP {status}: {detail}"))
    }

    /// GET a JSON resource; non-2xx is an [`AzureError::Api`]. `url` may
    /// be a path or an absolute URL (LRO poll).
    pub async fn get(&self, url: &str, desc: &str) -> Result<Value, AzureError> {
        let response = self.send(reqwest::Method::GET, url, None).await?;
        if !response.status().is_success() {
            return Err(Self::api_error(response, desc).await);
        }
        let text = response.text().await.unwrap_or_default();
        serde_json::from_str(&text)
            .map_err(|err| AzureError::Api(format!("Azure {desc} -> invalid JSON: {err}")))
    }

    /// GET that maps 404 to `None` (Python's `except ResourceNotFoundError`).
    pub async fn get_allow_404(&self, path: &str, desc: &str) -> Result<Option<Value>, AzureError> {
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
            .map_err(|err| AzureError::Api(format!("Azure {desc} -> invalid JSON: {err}")))
    }

    /// DELETE a resource; `false` on 404 (idempotent NotFound). Does NOT
    /// wait for the returned operation — Python's `begin_delete` call
    /// never invokes `op.result()` either.
    pub async fn delete_allow_404(&self, path: &str, desc: &str) -> Result<bool, AzureError> {
        let response = self.send(reqwest::Method::DELETE, path, None).await?;
        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(false);
        }
        if !response.status().is_success() {
            return Err(Self::api_error(response, desc).await);
        }
        Ok(true)
    }

    /// PUT a resource body and wait for the async operation to reach a
    /// terminal state (Python SDK `op.result()`). Returns the parsed PUT
    /// response body.
    pub async fn put_lro(&self, path: &str, body: &Value, desc: &str) -> Result<Value, AzureError> {
        let response = self.send(reqwest::Method::PUT, path, Some(body)).await?;
        if !response.status().is_success() {
            return Err(Self::api_error(response, desc).await);
        }
        let header = |name: &str| {
            response
                .headers()
                .get(name)
                .and_then(|v| v.to_str().ok())
                .map(str::to_string)
        };
        let async_op = header("azure-asyncoperation");
        let location = header("location");
        let text = response.text().await.unwrap_or_default();
        if let Some(url) = async_op {
            self.poll_async_operation(&url, desc).await?;
        } else if let Some(url) = location {
            self.poll_location(&url, desc).await?;
        }
        serde_json::from_str(&text)
            .map_err(|err| AzureError::Api(format!("Azure {desc} -> invalid JSON: {err}")))
    }

    /// Poll an Azure-AsyncOperation URL until Succeeded/Failed/Canceled.
    async fn poll_async_operation(&self, url: &str, desc: &str) -> Result<(), AzureError> {
        loop {
            let body = self.get(url, &format!("poll {desc}")).await?;
            let status = body.get("status").and_then(Value::as_str).unwrap_or("");
            match status {
                "Succeeded" => return Ok(()),
                "Failed" | "Canceled" => {
                    let error = body.get("error").cloned().unwrap_or(Value::Null);
                    let code = error.get("code").and_then(Value::as_str).unwrap_or("");
                    let message = error.get("message").and_then(Value::as_str).unwrap_or("");
                    return Err(AzureError::Api(format!(
                        "Azure {desc} operation {status}: {}",
                        format!("{code} {message}").trim()
                    )));
                }
                _ => tokio::time::sleep(self.inner.poll_interval).await,
            }
        }
    }

    /// Poll a Location header URL until it stops returning 202.
    async fn poll_location(&self, url: &str, desc: &str) -> Result<(), AzureError> {
        loop {
            let response = self.send(reqwest::Method::GET, url, None).await?;
            if response.status() == reqwest::StatusCode::ACCEPTED {
                tokio::time::sleep(self.inner.poll_interval).await;
                continue;
            }
            if !response.status().is_success() {
                return Err(Self::api_error(response, desc).await);
            }
            return Ok(());
        }
    }

    /// GET a VM with `$expand=instanceView`; None on 404.
    pub async fn get_vm(&self, rg: &str, name: &str) -> Result<Option<Value>, AzureError> {
        let path = format!(
            "{}?$expand=instanceView&api-version={COMPUTE_API_VERSION}",
            vm_path(self.subscription(), rg, name)
        );
        self.get_allow_404(&path, &format!("get VM {name}")).await
    }

    /// List VMs in the resource group (nextLink-paginated), as raw JSON.
    pub async fn list_vms(&self, rg: &str) -> Result<Vec<Value>, AzureError> {
        let mut out = Vec::new();
        let mut url = format!(
            "/subscriptions/{}/resourceGroups/{rg}\
             /providers/Microsoft.Compute/virtualMachines?api-version={COMPUTE_API_VERSION}",
            self.subscription()
        );
        loop {
            let page = self.get(&url, "list virtualMachines").await?;
            if let Some(vms) = page.get("value").and_then(Value::as_array) {
                out.extend(vms.iter().cloned());
            }
            match page.get("nextLink").and_then(Value::as_str) {
                Some(next) => url = next.to_string(),
                None => break,
            }
        }
        Ok(out)
    }
    /// List regional Microsoft.Compute quota usages.
    pub async fn list_usages(&self, location: &str) -> Result<Vec<Value>, AzureError> {
        let path = format!(
            "/subscriptions/{}/providers/Microsoft.Compute/locations/{location}\
             /usages?api-version={COMPUTE_API_VERSION}",
            self.subscription()
        );
        let page = self
            .get(&path, &format!("list compute usages in {location}"))
            .await?;
        Ok(page
            .get("value")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default())
    }
}

/// ARM resource path of a VM (no api-version). Crate-visible for the
/// agent's self-delete ([`crate::providers::local::azure_self`]).
pub(crate) fn vm_path(subscription: &str, rg: &str, name: &str) -> String {
    format!(
        "/subscriptions/{subscription}\
         /resourceGroups/{rg}\
         /providers/Microsoft.Compute/virtualMachines/{name}"
    )
}

// --- Pure builders + classification (split out for tests) ---

/// Python `_parse_image_urn`. Err is the Python ValueError.
pub fn parse_image_urn(urn: &str) -> Result<Value, String> {
    let parts: Vec<&str> = urn.split(':').collect();
    if parts.len() != 4 {
        return Err(format!(
            "AZURE_IMAGE_URN must be 'publisher:offer:sku:version', got {urn:?}"
        ));
    }
    Ok(json!({
        "publisher": parts[0],
        "offer": parts[1],
        "sku": parts[2],
        "version": parts[3],
    }))
}

/// Python's VM body from `create_instance`. Split out pure for tests.
/// Err is the Python ValueError from `_parse_image_urn`.
///
/// Deviation from Python: a non-empty `identity_id` also renders the ARM
/// `identity` block, which is what gives the VM a token source (IMDS) for
/// the blob queue and for deleting itself. Empty renders the Python body
/// unchanged.
#[allow(clippy::too_many_arguments)]
pub fn vm_body(
    name: &str,
    location: &str,
    machine_type: &str,
    boot_disk_gb: i64,
    image_urn: &str,
    username: &str,
    ssh_public_key: &str,
    startup_script: &str,
    nic_id: &str,
    identity_id: &str,
    preemptible: bool,
) -> Result<Value, String> {
    let image_reference = parse_image_urn(image_urn)?;
    let ssh = if ssh_public_key.is_empty() {
        json!({})
    } else {
        json!({
            "publicKeys": [{
                "path": format!("/home/{username}/.ssh/authorized_keys"),
                "keyData": ssh_public_key,
            }],
        })
    };
    let mut properties = json!({
        "hardwareProfile": { "vmSize": machine_type },
        "storageProfile": {
            "imageReference": image_reference,
            "osDisk": {
                "createOption": "FromImage",
                "diskSizeGB": boot_disk_gb,
                "managedDisk": { "storageAccountType": "Premium_LRS" },
                "deleteOption": "Delete",
            },
        },
        "osProfile": {
            // Azure caps Linux hostname at 15.
            "computerName": name.chars().take(15).collect::<String>(),
            "adminUsername": username,
            "customData": base64::engine::general_purpose::STANDARD
                .encode(startup_script.as_bytes()),
            "linuxConfiguration": {
                "disablePasswordAuthentication": true,
                "ssh": ssh,
            },
        },
        "networkProfile": {
            "networkInterfaces": [{
                "id": nic_id,
                "properties": { "primary": true, "deleteOption": "Delete" },
            }],
        },
    });
    if preemptible {
        // Azure Spot: priority="Spot", eviction_policy="Delete" so a
        // preempted VM is fully removed (matches GCP's
        // instance_termination_action="DELETE"). billing_profile
        // max_price=-1 means "pay up to on-demand list price", i.e. take
        // whatever Spot capacity is available without an explicit cap.
        // The scheduler enforces cost via max_cost_per_hour_usd
        // separately.
        properties["priority"] = json!("Spot");
        properties["evictionPolicy"] = json!("Delete");
        properties["billingProfile"] = json!({ "maxPrice": -1.0 });
    }
    let mut body = json!({
        "location": location,
        "tags": {
            "wisent_managed": "true",
            "wisent_created": chrono::Utc::now()
                .to_rfc3339_opts(chrono::SecondsFormat::Micros, false),
        },
        "properties": properties,
    });
    if !identity_id.is_empty() {
        // ARM hangs `identity` off the resource root, beside `location` and
        // `properties`, not inside them. userAssignedIdentities is a map
        // keyed by the identity's resource id whose value ARM fills in with
        // principalId/clientId, so we send an empty object. Skipped when
        // unconfigured, keeping the rendered body byte-identical to the
        // Python original.
        body["identity"] = json!({
            "type": "UserAssigned",
            "userAssignedIdentities": { identity_id: {} },
        });
    }
    Ok(body)
}

/// Python NIC-create failure classification: QuotaExceeded /
/// OperationNotAllowed mark the location as skipped.
fn nic_skip_error(msg: &str) -> bool {
    msg.contains("QuotaExceeded") || msg.contains("OperationNotAllowed")
}

/// Python VM-create failure classification adds SkuNotAvailable.
fn vm_skip_error(msg: &str) -> bool {
    nic_skip_error(msg) || msg.contains("SkuNotAvailable")
}

/// First `PowerState/...` code of the instanceView ("running",
/// "deallocated", ...), None when absent.
pub fn power_state(vm: &Value) -> Option<String> {
    let statuses = vm
        .get("properties")?
        .get("instanceView")?
        .get("statuses")?
        .as_array()?;
    for status in statuses {
        if let Some(code) = status.get("code").and_then(Value::as_str) {
            if let Some(state) = code.strip_prefix("PowerState/") {
                return Some(state.to_string());
            }
        }
    }
    None
}

/// Python `instance_exists` mapping (provisioning_state already
/// lowercased here; power_state is the raw string after `PowerState/`).
pub fn vm_is_alive(provisioning_state: Option<&str>, power_state: Option<&str>) -> bool {
    // provisioningState == "Succeeded" + power_state in (running,
    // starting) is the closest analogue to GCE
    // RUNNING/STAGING/PROVISIONING. Azure also has "Updating", which we
    // treat as alive — a VM mid-update is still consuming GPU quota and
    // shouldn't be requeued.
    let prov = provisioning_state.unwrap_or("").to_lowercase();
    if matches!(prov.as_str(), "creating" | "updating" | "succeeded") {
        if let Some(state) = power_state {
            return matches!(state, "running" | "starting");
        }
        // Mid-create: no PowerState yet — treat as alive.
        return matches!(prov.as_str(), "creating" | "updating");
    }
    false
}

// --- Provider ---

/// Resolved-at-first-use provider state (see the module deviation note).
struct AzureState {
    client: ArmClient,
}

/// Python `AzureProvider`.
pub struct AzureProvider {
    state: OnceCell<AzureState>,
}

impl AzureProvider {
    /// Python `AzureProvider()` — lazy in Rust (see the module docs).
    pub fn from_env() -> Self {
        AzureProvider {
            state: OnceCell::new(),
        }
    }

    /// Bind an explicit client (tests).
    #[cfg(test)]
    pub(crate) fn with_client(client: ArmClient) -> Self {
        let state = OnceCell::new();
        let _ = state.set(AzureState { client });
        AzureProvider { state }
    }

    async fn state(&self) -> Result<&AzureState, ProviderError> {
        self.state
            .get_or_try_init(|| async {
                let subscription = config::azure_subscription_id();
                if subscription.is_empty() {
                    // Python raises RuntimeError at construction; deferred
                    // to first use here (see the module docs).
                    return Err(ProviderError::Value(
                        "AZURE_SUBSCRIPTION_ID env var is empty; cannot construct AzureProvider"
                            .to_string(),
                    ));
                }
                Ok::<_, ProviderError>(AzureState {
                    client: ArmClient::new(subscription),
                })
            })
            .await
    }

    /// Python's `name@location` ref builder.
    fn reference(name: &str, location: &str) -> String {
        format!("{name}@{location}")
    }

    /// Python `name, _ = instance_ref.split("@")` — a ref that does not
    /// split into exactly two parts is a ValueError.
    fn parse_ref(instance_ref: &str) -> Result<&str, ProviderError> {
        let parts: Vec<&str> = instance_ref.split('@').collect();
        if parts.len() != 2 {
            return Err(ProviderError::Value(format!(
                "invalid instance_ref (expected name@location): {instance_ref}"
            )));
        }
        Ok(parts[0])
    }

    /// Python `list_running_instance_refs_with_age`: `(name@location,
    /// age_in_seconds)` for live `<prefix>-agent-*` VMs.
    ///
    /// Mirrors providers/gcp — restricts to '<prefix>-agent-*' so the
    /// dead-agent reaper doesn't sweep unrelated wisent-* VMs.
    pub async fn list_running_instance_refs_with_age(
        &self,
    ) -> Result<Vec<(String, f64)>, ProviderError> {
        let state = self.state().await?;
        let vms = state
            .client
            .list_vms(config::azure_resource_group())
            .await?;
        let prefix = format!("{}-agent-", config::INSTANCE_PREFIX);
        let now = chrono::Utc::now();
        let mut out = Vec::new();
        for vm in &vms {
            let name = vm.get("name").and_then(Value::as_str).unwrap_or("");
            if !name.starts_with(&prefix) {
                continue;
            }
            let created = vm
                .get("tags")
                .and_then(|t| t.get("wisent_created"))
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
            let location = vm.get("location").and_then(Value::as_str).unwrap_or("");
            out.push((format!("{name}@{location}"), age));
        }
        Ok(out)
    }

    /// Python `list_running_instance_refs`.
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
impl Provider for AzureProvider {
    #[allow(clippy::too_many_arguments)]
    async fn create_instance(
        &self,
        name: &str,
        machine_type: &str,
        _accel_type: &str,
        boot_disk_gb: i64,
        _image: &str,
        _image_project: &str,
        startup_script: &str,
        preemptible: bool,
    ) -> Result<Option<String>, ProviderError> {
        let state = self.state().await?;
        let client = &state.client;
        let rg = config::azure_resource_group();
        let mut skipped: HashSet<String> = HashSet::new();
        for location in config::azure_locations() {
            if skipped.contains(location) {
                continue;
            }
            let subnet = network::subnet_id(
                client.subscription(),
                rg,
                config::azure_vnet(),
                config::azure_subnet(),
                location,
            );
            let nsg = network::nsg_id(client.subscription(), rg, config::azure_nsg(), location);
            let nic_id = match network::create_nic(client, rg, name, location, &subnet, &nsg).await
            {
                Ok(id) => id,
                Err(err) => {
                    let msg = err.to_string();
                    log(&format!("NIC create failed in {location}: {err}"));
                    if nic_skip_error(&msg) {
                        skipped.insert(location.clone());
                    }
                    continue;
                }
            };

            let body = vm_body(
                name,
                location,
                machine_type,
                boot_disk_gb,
                config::azure_image_urn(),
                config::azure_vm_username(),
                config::azure_ssh_public_key(),
                startup_script,
                &nic_id,
                config::azure_vm_identity_id(),
                preemptible,
            )
            .map_err(ProviderError::Value)?;
            let path = format!(
                "{}?api-version={COMPUTE_API_VERSION}",
                vm_path(client.subscription(), rg, name)
            );
            match client
                .put_lro(&path, &body, &format!("create VM {name}@{location}"))
                .await
            {
                Ok(_) => {
                    log(&format!(
                        "Created {} preemptible={}",
                        Self::reference(name, location),
                        py_bool(preemptible)
                    ));
                    return Ok(Some(Self::reference(name, location)));
                }
                Err(err) => {
                    let msg = err.to_string();
                    if msg.to_lowercase().contains("already exists") {
                        return Ok(Some(Self::reference(name, location)));
                    }
                    log(&format!("VM create failed in {location}: {err}"));
                    // Roll back the NIC we just created so we don't leak
                    // it.
                    network::delete_nic(client, rg, name).await;
                    if vm_skip_error(&msg) {
                        skipped.insert(location.clone());
                    }
                    continue;
                }
            }
        }
        Ok(None)
    }

    async fn delete_instance(&self, instance_ref: &str) -> Result<(), ProviderError> {
        let state = self.state().await?;
        let name = Self::parse_ref(instance_ref)?;
        let rg = config::azure_resource_group();
        let path = format!(
            "{}?api-version={COMPUTE_API_VERSION}",
            vm_path(state.client.subscription(), rg, name)
        );
        // Idempotent: already gone is the desired terminal state.
        state
            .client
            .delete_allow_404(&path, &format!("delete VM {name}"))
            .await?;
        // NIC cleanup mirrors the VM-delete contract: NotFound is
        // idempotent success, and failures are best-effort log-only
        // inside delete_nic (Python network.py swallows all exceptions).
        network::delete_nic(&state.client, rg, name).await;
        Ok(())
    }

    async fn instance_exists(&self, instance_ref: &str) -> Result<bool, ProviderError> {
        let state = self.state().await?;
        let name = Self::parse_ref(instance_ref)?;
        let Some(vm) = state
            .client
            .get_vm(config::azure_resource_group(), name)
            .await?
        else {
            return Ok(false);
        };
        let prov = vm
            .get("properties")
            .and_then(|p| p.get("provisioningState"))
            .and_then(Value::as_str);
        Ok(vm_is_alive(prov, power_state(&vm).as_deref()))
    }

    /// Return the literal Azure power-state ('running', 'deallocated',
    /// ...).
    ///
    /// The monitor uses lifecycle_state == "TERMINATED" (GCE) to detect
    /// Spot preemption. On Azure, Spot eviction lands the VM in
    /// PowerState/deallocated — the monitor treats that string as the
    /// preemption signal.
    async fn instance_lifecycle_state(
        &self,
        instance_ref: &str,
    ) -> Result<Option<String>, ProviderError> {
        let state = self.state().await?;
        let name = Self::parse_ref(instance_ref)?;
        let Some(vm) = state
            .client
            .get_vm(config::azure_resource_group(), name)
            .await?
        else {
            return Ok(None);
        };
        Ok(power_state(&vm))
    }

    /// Trait override delegating to the inherent method (kept for direct
    /// AzureProvider callers) so `&dyn Provider` consumers — the dead-agent
    /// reaper and `cli/instances.rs` — can reach it. Without this the base
    /// default applied and every Azure agent VM was invisible to both.
    /// Mirrors providers/gcp.
    async fn list_running_instance_refs_with_age(
        &self,
    ) -> Result<Vec<(String, f64)>, ProviderError> {
        AzureProvider::list_running_instance_refs_with_age(self).await
    }

    /// `{accel_type: count}` for all live wisent-* VMs across the
    /// resource group.
    async fn list_running_instances(&self) -> Result<BTreeMap<String, i64>, ProviderError> {
        let state = self.state().await?;
        let vms = state
            .client
            .list_vms(config::azure_resource_group())
            .await?;
        let mut counts: BTreeMap<String, i64> = BTreeMap::new();
        let prefix = format!("{}-", config::INSTANCE_PREFIX);
        for vm in &vms {
            let name = vm.get("name").and_then(Value::as_str).unwrap_or("");
            if !name.starts_with(&prefix) {
                continue;
            }
            // Cheap state probe: list() doesn't include instance_view by
            // default, so trust the tag we stamped at create-time. A VM
            // in the resource group with the wisent_managed tag and a
            // known GPU SKU consumes quota until we delete it; counting
            // it as running is the safe direction.
            let sku = vm
                .get("properties")
                .and_then(|p| p.get("hardwareProfile"))
                .and_then(|h| h.get("vmSize"))
                .and_then(Value::as_str)
                .unwrap_or("");
            let Some((accel, n)) = AZURE_VM_TO_ACCEL.get(sku) else {
                continue;
            };
            *counts.entry((*accel).to_string()).or_insert(0) += n;
        }
        Ok(counts)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{http_response, mock_http, MockHttp};

    /// Response with extra headers (LRO polls need Azure-AsyncOperation).
    fn response_with(status: u16, reason: &str, headers: &[(&str, &str)], body: &str) -> String {
        let extra: String = headers
            .iter()
            .map(|(k, v)| format!("{k}: {v}\r\n"))
            .collect();
        format!(
            "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\n\
             Content-Length: {}\r\nConnection: close\r\n{extra}\r\n{body}",
            body.len()
        )
    }

    fn provider_for(server: &MockHttp) -> AzureProvider {
        AzureProvider::with_client(ArmClient::for_test(&server.base_url, "sub-1"))
    }

    fn request_bodies(server: &MockHttp) -> Vec<String> {
        server.requests.lock().unwrap().clone()
    }

    #[test]
    fn parse_image_urn_shape_and_error() {
        let urn = parse_image_urn("microsoft-dsvm:ubuntu-hpc:2204:latest").unwrap();
        assert_eq!(
            urn,
            json!({
                "publisher": "microsoft-dsvm",
                "offer": "ubuntu-hpc",
                "sku": "2204",
                "version": "latest",
            })
        );
        let err = parse_image_urn("too:few").unwrap_err();
        assert!(
            err.contains("AZURE_IMAGE_URN must be 'publisher:offer:sku:version'"),
            "{err}"
        );
    }

    #[test]
    fn vm_body_spot_and_on_demand_shapes() {
        let spot = vm_body(
            "wisent-agent-a100-1778891822-0",
            "eastus",
            "Standard_NC8ads_A10_v4",
            200,
            "microsoft-dsvm:ubuntu-hpc:2204:latest",
            "wisent",
            "ssh-ed25519 AAAA",
            "#!/bin/bash\necho hi",
            "/nic/id",
            "",
            true,
        )
        .unwrap();
        let props = &spot["properties"];
        assert_eq!(spot["location"], json!("eastus"));
        assert_eq!(spot["tags"]["wisent_managed"], json!("true"));
        assert!(spot["tags"]["wisent_created"]
            .as_str()
            .unwrap()
            .contains('+'));
        // No identity id configured -> no identity block at all, so the
        // body stays byte-identical to the pre-identity shape.
        assert!(spot.get("identity").is_none());
        // Azure caps Linux hostname at 15.
        assert_eq!(props["osProfile"]["computerName"], json!("wisent-agent-a1"));
        assert_eq!(
            props["hardwareProfile"]["vmSize"],
            json!("Standard_NC8ads_A10_v4")
        );
        assert_eq!(props["storageProfile"]["osDisk"]["diskSizeGB"], json!(200));
        assert_eq!(
            props["storageProfile"]["osDisk"]["managedDisk"]["storageAccountType"],
            json!("Premium_LRS")
        );
        assert_eq!(
            props["storageProfile"]["osDisk"]["deleteOption"],
            json!("Delete")
        );
        assert_eq!(
            props["storageProfile"]["imageReference"],
            json!({"publisher": "microsoft-dsvm", "offer": "ubuntu-hpc", "sku": "2204", "version": "latest"})
        );
        assert_eq!(
            props["osProfile"]["linuxConfiguration"]["ssh"]["publicKeys"][0]["path"],
            json!("/home/wisent/.ssh/authorized_keys")
        );
        assert_eq!(
            props["osProfile"]["linuxConfiguration"]["ssh"]["publicKeys"][0]["keyData"],
            json!("ssh-ed25519 AAAA")
        );
        let expected_custom =
            base64::engine::general_purpose::STANDARD.encode("#!/bin/bash\necho hi".as_bytes());
        assert_eq!(props["osProfile"]["customData"], json!(expected_custom));
        assert_eq!(
            props["networkProfile"]["networkInterfaces"][0],
            json!({"id": "/nic/id", "properties": {"primary": true, "deleteOption": "Delete"}})
        );
        // Spot fields.
        assert_eq!(props["priority"], json!("Spot"));
        assert_eq!(props["evictionPolicy"], json!("Delete"));
        assert_eq!(props["billingProfile"], json!({ "maxPrice": -1.0 }));

        // On-demand: no Spot fields; empty ssh key -> empty ssh object.
        let on_demand = vm_body(
            "vm1",
            "westus3",
            "Standard_NC6",
            config::DEFAULT_BOOT_DISK_GB,
            "a:b:c:d",
            "wisent",
            "",
            "",
            "/nic/id2",
            "",
            false,
        )
        .unwrap();
        let p2 = &on_demand["properties"];
        assert!(p2.get("priority").is_none());
        assert!(p2.get("evictionPolicy").is_none());
        assert!(p2.get("billingProfile").is_none());
        assert_eq!(p2["osProfile"]["linuxConfiguration"]["ssh"], json!({}));
        assert_eq!(p2["osProfile"]["computerName"], json!("vm1"));
    }

    #[test]
    fn vm_body_renders_user_assigned_identity_block() {
        // One operator-provisioned identity, reused by every ephemeral agent
        // VM; on the VM it is the only thing IMDS can hand a token for.
        let identity = "/subscriptions/sub/resourceGroups/wisent-compute/providers/\
                        Microsoft.ManagedIdentity/userAssignedIdentities/wisent-agent";
        let body = vm_body(
            "vm1",
            "eastus",
            "Standard_NC6",
            config::DEFAULT_BOOT_DISK_GB,
            "a:b:c:d",
            "wisent",
            "",
            "",
            "/nic/id",
            identity,
            false,
        )
        .unwrap();
        // Sibling of location/properties, not nested inside either, and the
        // identity map's value is the empty object ARM populates.
        assert_eq!(
            body["identity"],
            json!({"type": "UserAssigned", "userAssignedIdentities": {identity: {}}})
        );
        assert!(body["properties"].get("identity").is_none());
    }

    #[test]
    fn skip_error_classification_on_fabricated_arm_messages() {
        // Fabricated ARM error payloads as embedded by api_error.
        let quota = "Azure create VM vm1@eastus -> HTTP 403: QuotaExceeded \
                     Operation could not be completed as it results in exceeding quota";
        let op_not_allowed = "Azure create NIC vm1-nic -> HTTP 403: OperationNotAllowed \
                              Your subscription does not have access to this SKU";
        let sku = "Azure create VM vm1@eastus operation Failed: SkuNotAvailable \
                   The requested VM size is not available";
        let conflict = "Azure create VM vm1@eastus -> HTTP 409: OperationNotAllowed xy";
        let other = "Azure create VM vm1@eastus -> HTTP 500: InternalServerError boom";

        assert!(nic_skip_error(quota));
        assert!(nic_skip_error(op_not_allowed));
        assert!(!nic_skip_error(sku)); // NIC classification lacks SkuNotAvailable
        assert!(vm_skip_error(quota));
        assert!(vm_skip_error(op_not_allowed));
        assert!(vm_skip_error(sku));
        assert!(vm_skip_error(conflict)); // OperationNotAllowed regardless of verb
        assert!(!vm_skip_error(other));
    }

    #[test]
    fn vm_is_alive_power_state_mapping() {
        // (provisioning_state, power_state) -> alive, per Python.
        assert!(vm_is_alive(Some("Succeeded"), Some("running")));
        assert!(vm_is_alive(Some("Succeeded"), Some("starting")));
        assert!(!vm_is_alive(Some("Succeeded"), Some("deallocated")));
        assert!(!vm_is_alive(Some("Succeeded"), Some("deallocating")));
        assert!(!vm_is_alive(Some("Succeeded"), Some("stopped")));
        // Mid-create / mid-update with no PowerState yet: alive.
        assert!(vm_is_alive(Some("Creating"), None));
        assert!(vm_is_alive(Some("Updating"), None));
        // Succeeded with no PowerState at all: not alive.
        assert!(!vm_is_alive(Some("Succeeded"), None));
        // Terminal provisioning states: not alive regardless.
        assert!(!vm_is_alive(Some("Failed"), Some("running")));
        assert!(!vm_is_alive(Some("Deallocated"), Some("running")));
        assert!(!vm_is_alive(None, None));
        // Case-insensitive provisioningState (Python lower()).
        assert!(vm_is_alive(Some("SUCCEEDED"), Some("running")));
    }

    #[test]
    fn power_state_extracts_first_code() {
        let vm = json!({
            "properties": {
                "instanceView": {
                    "statuses": [
                        {"code": "ProvisioningState/succeeded"},
                        {"code": "PowerState/deallocated"},
                        {"code": "PowerState/running"},
                    ],
                },
            },
        });
        assert_eq!(power_state(&vm).as_deref(), Some("deallocated"));
        assert_eq!(power_state(&json!({"properties": {}})), None);
        assert_eq!(power_state(&json!({})), None);
    }

    #[tokio::test]
    async fn create_instance_quota_fallthrough_to_second_location() {
        let server = mock_http(vec![
            // eastus: NIC PUT rejected synchronously with QuotaExceeded.
            http_response(
                403,
                "Forbidden",
                r#"{"error": {"code": "QuotaExceeded", "message": "Operation could not be completed as it results in exceeding approved Total Regional Cores quota."}}"#,
            ),
            // westus3: NIC PUT accepted, async op -> Succeeded.
            response_with(
                201,
                "Created",
                &[("Azure-AsyncOperation", "/operations/nic-op")],
                r#"{"id": "/subscriptions/sub-1/resourceGroups/rg/providers/Microsoft.Network/networkInterfaces/vm1-nic"}"#,
            ),
            http_response(200, "OK", r#"{"status": "Succeeded"}"#),
            // westus3: VM PUT accepted, async op -> Succeeded.
            response_with(
                201,
                "Created",
                &[("Azure-AsyncOperation", "/operations/vm-op")],
                r#"{"id": "/subscriptions/sub-1/.../virtualMachines/vm1"}"#,
            ),
            http_response(200, "OK", r#"{"status": "Succeeded"}"#),
        ])
        .await;
        let provider = provider_for(&server);
        let result = provider
            .create_instance(
                "vm1",
                "Standard_NC8ads_A10_v4",
                "nvidia-a10",
                200,
                "",
                "",
                "echo hi",
                true,
            )
            .await
            .unwrap();
        assert_eq!(result.as_deref(), Some("vm1@westus3"));

        let requests = request_bodies(&server);
        assert_eq!(requests.len(), 5, "{requests:?}");
        assert!(
            requests[0].starts_with(
                "PUT /subscriptions/sub-1/resourceGroups/wisent-compute/providers/Microsoft.Network/networkInterfaces/vm1-nic?api-version="
            ),
            "{}",
            requests[0]
        );
        assert!(
            requests[0].contains("wisent-compute-vnet-eastus"),
            "{}",
            requests[0]
        );
        assert!(
            requests[1].starts_with(
                "PUT /subscriptions/sub-1/resourceGroups/wisent-compute/providers/Microsoft.Network/networkInterfaces/vm1-nic?api-version="
            ),
            "{}",
            requests[1]
        );
        assert!(
            requests[1].contains("wisent-compute-vnet-westus3"),
            "{}",
            requests[1]
        );
        assert!(
            requests[2].starts_with("GET /operations/nic-op "),
            "{}",
            requests[2]
        );
        assert!(
            requests[3].starts_with(
                "PUT /subscriptions/sub-1/resourceGroups/wisent-compute/providers/Microsoft.Compute/virtualMachines/vm1?api-version="
            ),
            "{}",
            requests[3]
        );
        // Spot fields in the VM body.
        assert!(
            requests[3].contains(r#""priority":"Spot""#),
            "{}",
            requests[3]
        );
        assert!(
            requests[3].contains(r#""maxPrice":-1.0"#),
            "{}",
            requests[3]
        );
        assert!(
            requests[4].starts_with("GET /operations/vm-op "),
            "{}",
            requests[4]
        );
        server.stop();
    }

    #[tokio::test]
    async fn create_instance_vm_failure_rolls_back_nic_and_continues() {
        let server = mock_http(vec![
            // eastus: NIC ok (sync 201), VM fails SkuNotAvailable, NIC deleted.
            http_response(
                201,
                "Created",
                r#"{"id": "/subscriptions/sub-1/.../networkInterfaces/vm1-nic"}"#,
            ),
            http_response(
                400,
                "Bad Request",
                r#"{"error": {"code": "SkuNotAvailable", "message": "The requested VM size Standard_NC8ads_A10_v4 is not available in eastus."}}"#,
            ),
            http_response(200, "OK", "{}"),
            // westus3: NIC ok, VM ok.
            http_response(
                201,
                "Created",
                r#"{"id": "/subscriptions/sub-1/.../networkInterfaces/vm1-nic"}"#,
            ),
            http_response(201, "Created", r#"{"id": "/subscriptions/sub-1/.../virtualMachines/vm1"}"#),
        ])
        .await;
        let provider = provider_for(&server);
        let result = provider
            .create_instance(
                "vm1",
                "Standard_NC8ads_A10_v4",
                "nvidia-a10",
                200,
                "",
                "",
                "echo hi",
                false,
            )
            .await
            .unwrap();
        assert_eq!(result.as_deref(), Some("vm1@westus3"));
        let requests = request_bodies(&server);
        assert_eq!(requests.len(), 5, "{requests:?}");
        // The rollback DELETE targets the eastus NIC before westus3 starts.
        assert!(
            requests[2].starts_with(
                "DELETE /subscriptions/sub-1/resourceGroups/wisent-compute/providers/Microsoft.Network/networkInterfaces/vm1-nic?api-version="
            ),
            "{}",
            requests[2]
        );
        server.stop();
    }

    #[tokio::test]
    async fn create_instance_all_locations_fail_returns_none() {
        let server = mock_http(vec![
            http_response(
                500,
                "Server Error",
                r#"{"error": {"code": "InternalError", "message": "boom"}}"#,
            ),
            http_response(
                500,
                "Server Error",
                r#"{"error": {"code": "InternalError", "message": "boom"}}"#,
            ),
            http_response(
                500,
                "Server Error",
                r#"{"error": {"code": "InternalError", "message": "boom"}}"#,
            ),
            http_response(
                500,
                "Server Error",
                r#"{"error": {"code": "InternalError", "message": "boom"}}"#,
            ),
        ])
        .await;
        let provider = provider_for(&server);
        let result = provider
            .create_instance("vm1", "Standard_NC6", "", 100, "", "", "", false)
            .await
            .unwrap();
        assert_eq!(result, None);
        server.stop();
    }

    #[tokio::test]
    async fn delete_instance_404_is_success_and_cleans_nic() {
        let server = mock_http(vec![
            http_response(
                404,
                "Not Found",
                r#"{"error": {"code": "ResourceNotFound", "message": "gone"}}"#,
            ),
            http_response(200, "OK", "{}"),
        ])
        .await;
        let provider = provider_for(&server);
        provider.delete_instance("vm1@eastus").await.unwrap();
        let requests = request_bodies(&server);
        assert_eq!(requests.len(), 2, "{requests:?}");
        assert!(
            requests[0].starts_with(
                "DELETE /subscriptions/sub-1/resourceGroups/wisent-compute/providers/Microsoft.Compute/virtualMachines/vm1?api-version="
            ),
            "{}",
            requests[0]
        );
        assert!(
            requests[1].contains("networkInterfaces/vm1-nic"),
            "{}",
            requests[1]
        );
        server.stop();

        // Malformed ref is a ValueError before any API call.
        let provider2 = provider_for(&server);
        let err = provider2.delete_instance("no-at-sign").await.unwrap_err();
        assert!(err.to_string().contains("expected name@location"), "{err}");
    }

    #[tokio::test]
    async fn instance_exists_and_lifecycle_state_mapping() {
        let vm_body_running = r#"{"name": "vm1", "properties": {"provisioningState": "Succeeded",
            "instanceView": {"statuses": [{"code": "ProvisioningState/succeeded"}, {"code": "PowerState/running"}]}}}"#;
        let server = mock_http(vec![
            http_response(200, "OK", vm_body_running),
            http_response(
                200,
                "OK",
                &vm_body_running.replace("PowerState/running", "PowerState/deallocated"),
            ),
            http_response(
                404,
                "Not Found",
                r#"{"error": {"code": "ResourceNotFound"}}"#,
            ),
            http_response(
                200,
                "OK",
                &vm_body_running.replace("PowerState/running", "PowerState/deallocated"),
            ),
        ])
        .await;
        let provider = provider_for(&server);
        assert!(provider.instance_exists("vm1@eastus").await.unwrap());
        assert!(!provider.instance_exists("vm1@eastus").await.unwrap());
        assert!(!provider.instance_exists("vm1@eastus").await.unwrap()); // 404
        let state = provider
            .instance_lifecycle_state("vm1@eastus")
            .await
            .unwrap();
        assert_eq!(state.as_deref(), Some("deallocated"));
        let requests = request_bodies(&server);
        assert!(
            requests[0].contains("$expand=instanceView"),
            "{}",
            requests[0]
        );
        server.stop();
    }

    #[tokio::test]
    async fn list_running_instances_counts_known_skus() {
        let page = r#"{"value": [
            {"name": "wisent-a", "location": "eastus", "properties": {"hardwareProfile": {"vmSize": "Standard_NC8ads_A10_v4"}}},
            {"name": "wisent-b", "location": "eastus", "properties": {"hardwareProfile": {"vmSize": "Standard_NC64as_T4_v3"}}},
            {"name": "unrelated-vm", "location": "eastus", "properties": {"hardwareProfile": {"vmSize": "Standard_NC8ads_A10_v4"}}},
            {"name": "wisent-c", "location": "eastus", "properties": {"hardwareProfile": {"vmSize": "Standard_D2s_v3"}}}
        ]}"#;
        let server = mock_http(vec![http_response(200, "OK", page)]).await;
        let provider = provider_for(&server);
        let counts = provider.list_running_instances().await.unwrap();
        assert_eq!(
            counts,
            BTreeMap::from([
                ("nvidia-a10".to_string(), 1),
                ("nvidia-tesla-t4".to_string(), 4),
            ])
        );
        server.stop();
    }

    #[tokio::test]
    async fn list_running_instance_refs_with_age_filters_agent_prefix() {
        let page = r#"{"value": [
            {"name": "wisent-agent-a100-1-0", "location": "eastus", "tags": {"wisent_created": "2026-07-25T19:00:00+00:00"}},
            {"name": "wisent-mig-api-1", "location": "eastus", "tags": {}}
        ]}"#;
        let server = mock_http(vec![http_response(200, "OK", page)]).await;
        let provider = provider_for(&server);
        let refs = provider
            .list_running_instance_refs_with_age()
            .await
            .unwrap();
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].0, "wisent-agent-a100-1-0@eastus");
        assert!(refs[0].1 > 0.0, "age should be positive: {:?}", refs[0]);
        server.stop();
    }
}
