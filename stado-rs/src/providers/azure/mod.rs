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
const VM_EXTENSION_API_VERSION: &str = "2022-11-01";
const AGENT_GRANT_EXTENSION_NAME: &str = "stado-agent-grant";
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

    /// POST one JSON request and decode the JSON response. Resource Graph,
    /// Cost Management, and Monitor use POST query endpoints even for
    /// read-only operations; exposing the typed transport keeps their
    /// authentication path identical to VM lifecycle calls.
    pub async fn post_json(
        &self,
        path: &str,
        body: &Value,
        desc: &str,
    ) -> Result<Value, AzureError> {
        let response = self.send(reqwest::Method::POST, path, Some(body)).await?;
        if !response.status().is_success() {
            return Err(Self::api_error(response, desc).await);
        }
        let text = response.text().await.unwrap_or_default();
        serde_json::from_str(&text)
            .map_err(|err| AzureError::Api(format!("Azure {desc} -> invalid JSON: {err}")))
    }

    /// POST a lifecycle action whose successful ARM response may have an
    /// empty body (for example VM start/deallocate).
    pub async fn post_action(&self, path: &str, desc: &str) -> Result<(), AzureError> {
        let body = json!({});
        let response = self.send(reqwest::Method::POST, path, Some(&body)).await?;
        if !response.status().is_success() {
            return Err(Self::api_error(response, desc).await);
        }
        Ok(())
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

    /// DELETE a resource and wait for Azure's operation to finish. Protected
    /// extension deletion uses this stronger form so decrypted handler
    /// settings are removed before dispatch is reported successful.
    async fn delete_lro_allow_404(&self, path: &str, desc: &str) -> Result<bool, AzureError> {
        let response = self.send(reqwest::Method::DELETE, path, None).await?;
        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(false);
        }
        if !response.status().is_success() {
            return Err(Self::api_error(response, desc).await);
        }
        let header = |name: &str| {
            response
                .headers()
                .get(name)
                .and_then(|value| value.to_str().ok())
                .map(str::to_string)
        };
        let async_op = header("azure-asyncoperation");
        let location = header("location");
        if let Some(url) = async_op {
            self.poll_async_operation(&url, desc).await?;
        } else if let Some(url) = location {
            self.poll_location(&url, desc).await?;
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

fn vm_extension_path(subscription: &str, resource_group: &str, vm_name: &str) -> String {
    format!(
        "{}/extensions/{AGENT_GRANT_EXTENSION_NAME}?api-version={VM_EXTENSION_API_VERSION}",
        vm_path(subscription, resource_group, vm_name)
    )
}

/// Script carried only in Azure `protectedSettings`. It writes the opaque
/// grant atomically into `/run` without placing it in argv/stdout, truncates
/// its own handler copy, and leaves the tmpfs file for the Rust client's
/// first-read cache to erase.
fn protected_agent_grant_script(agent_grant: &str) -> String {
    let encoded_grant = base64::engine::general_purpose::STANDARD.encode(agent_grant.as_bytes());
    format!(
        r#"#!/bin/sh
set -eu
set +x
umask 077
grant_dir=/run/stado-agent-credentials
grant_file=$grant_dir/skarbiec-token
grant_tmp=
grant_b64='{encoded_grant}'
cleanup() {{
    grant_b64=
    if [ -n "${{grant_tmp:-}}" ]; then
        : > "$grant_tmp" 2>/dev/null || true
        rm -f "$grant_tmp"
    fi
    if [ -f "$0" ]; then
        : > "$0" 2>/dev/null || true
        rm -f "$0"
    fi
}}
trap cleanup EXIT HUP INT TERM
install -d -m 0700 "$grant_dir"
grant_tmp="$(mktemp "$grant_dir/.skarbiec-token.XXXXXX")"
printf '%s' "$grant_b64" | base64 -d > "$grant_tmp"
grant_b64=
chmod 0600 "$grant_tmp"
mv -f "$grant_tmp" "$grant_file"
grant_tmp=
"#
    )
}

fn agent_grant_extension_body(location: &str, agent_grant: &str) -> Value {
    let protected_script = base64::engine::general_purpose::STANDARD
        .encode(protected_agent_grant_script(agent_grant).as_bytes());
    json!({
        "location": location,
        "properties": {
            "publisher": "Microsoft.Azure.Extensions",
            "type": "CustomScript",
            "typeHandlerVersion": "2.1",
            "autoUpgradeMinorVersion": true,
            "enableAutomaticUpgrade": true,
            "settings": {},
            "protectedSettings": {
                "script": protected_script,
            },
        },
    })
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

    /// Parse the opaque `name@location` handle without allocating.
    fn parse_ref_parts(instance_ref: &str) -> Result<(&str, &str), ProviderError> {
        let Some((name, location)) = instance_ref.split_once('@') else {
            return Err(ProviderError::Value(format!(
                "invalid instance_ref (expected name@location): {instance_ref}"
            )));
        };
        if name.is_empty() || location.is_empty() || location.contains('@') {
            return Err(ProviderError::Value(format!(
                "invalid instance_ref (expected name@location): {instance_ref}"
            )));
        }
        Ok((name, location))
    }

    fn parse_ref(instance_ref: &str) -> Result<&str, ProviderError> {
        Self::parse_ref_parts(instance_ref).map(|(name, _)| name)
    }

    async fn install_agent_grant_extension(
        &self,
        instance_ref: &str,
        agent_grant: &str,
    ) -> Result<(), ProviderError> {
        if agent_grant.is_empty() {
            return Err(ProviderError::Value(
                "Azure protected agent grant is empty".to_string(),
            ));
        }
        let (name, location) = Self::parse_ref_parts(instance_ref)?;
        let state = self.state().await?;
        let path = vm_extension_path(
            state.client.subscription(),
            config::azure_resource_group(),
            name,
        );
        let body = agent_grant_extension_body(location, agent_grant);
        if let Err(error) = state
            .client
            .put_lro(
                &path,
                &body,
                &format!("deliver protected agent grant to VM {instance_ref}"),
            )
            .await
        {
            let _ = state
                .client
                .delete_lro_allow_404(
                    &path,
                    &format!("remove failed protected-grant extension from VM {instance_ref}"),
                )
                .await;
            return Err(error.into());
        }
        state
            .client
            .delete_lro_allow_404(
                &path,
                &format!("remove protected-grant extension from VM {instance_ref}"),
            )
            .await?;
        Ok(())
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
    async fn create_agent_instance(
        &self,
        name: &str,
        machine_type: &str,
        accel_type: &str,
        boot_disk_gb: i64,
        image: &str,
        image_project: &str,
        startup_script: &str,
        preemptible: bool,
        agent_grant: Option<&str>,
    ) -> Result<Option<String>, ProviderError> {
        let agent_grant = agent_grant
            .filter(|grant| !grant.is_empty())
            .ok_or_else(|| {
                ProviderError::Value(
                    "Azure agent creation requires protected-settings grant delivery".to_string(),
                )
            })?;
        let reference = self
            .create_instance(
                name,
                machine_type,
                accel_type,
                boot_disk_gb,
                image,
                image_project,
                startup_script,
                preemptible,
            )
            .await?;
        let Some(reference) = reference else {
            return Ok(None);
        };
        if let Err(error) = self
            .install_agent_grant_extension(&reference, agent_grant)
            .await
        {
            if let Err(cleanup_error) = self.delete_instance(&reference).await {
                log(&format!(
                    "protected agent-grant delivery failed for {reference}; VM cleanup also failed: {cleanup_error}"
                ));
            }
            return Err(error);
        }
        Ok(Some(reference))
    }

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

    async fn stop_instance(&self, instance_ref: &str) -> Result<(), ProviderError> {
        let state = self.state().await?;
        let name = Self::parse_ref(instance_ref)?;
        let path = format!(
            "{}/deallocate?api-version={COMPUTE_API_VERSION}",
            vm_path(
                state.client.subscription(),
                config::azure_resource_group(),
                name,
            )
        );
        state
            .client
            .post_action(&path, &format!("deallocate VM {name}"))
            .await?;
        Ok(())
    }

    async fn start_instance(&self, instance_ref: &str) -> Result<(), ProviderError> {
        let state = self.state().await?;
        let name = Self::parse_ref(instance_ref)?;
        let path = format!(
            "{}/start?api-version={COMPUTE_API_VERSION}",
            vm_path(
                state.client.subscription(),
                config::azure_resource_group(),
                name,
            )
        );
        state
            .client
            .post_action(&path, &format!("start VM {name}"))
            .await?;
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
