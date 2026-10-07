//! Crusoe Cloud virtual machines.
//!
//! API: <https://docs.crusoecloud.com/api>, signed per
//! <https://docs.crusoecloud.com/reference/api>: an HMAC-SHA256 over
//! `<path>\n<sorted query>\n<verb>\n<timestamp>\n` under the url-safe-base64
//! decoded secret key, sent as `Authorization: Bearer
//! <signature version>:<access key id>:<signature>` beside
//! `X-Crusoe-Timestamp`. `GET /v1/capacities` says whether a location has a
//! type to give; `POST /v1/projects/{project}/compute/vms/instances` takes
//! `name`, `type`, `location`, `image`, `ssh_public_key` and
//! `startup_script`; `DELETE .../instances/{id}` releases one; `GET
//! .../instances/{id}` and `GET .../instances` read them.

use async_trait::async_trait;
use base64::Engine as _;
use serde_json::{json, Value};

use crate::capabilities::{ConfigField, GpuCloudVendor};
use crate::providers::gpu_cloud::access::{required_setting, secret, setting};
use crate::providers::gpu_cloud::api::{
    GpuCloudApi, GpuCloudError, LaunchRequest, Machine, MachineState,
};
use crate::providers::gpu_cloud::http;
use crate::providers::gpu_cloud::vendors::signing;
use crate::providers::gpu_cloud::{GuestIdentity, VendorProfile};

const VENDOR: GpuCloudVendor = GpuCloudVendor::Crusoe;
const API: &str = "https://api.cloud.crusoe.ai";

pub const PROFILE: VendorProfile = VendorProfile {
    vendor: VENDOR,
    summary: "Rent Crusoe Cloud GPU virtual machines and run an agent on each.",
    config: &[
        ConfigField::scalar("project-id", "CRUSOE_PROJECT_ID", "crusoe.project_id").required(),
        ConfigField::scalar("location", "CRUSOE_LOCATION", "crusoe.location").required(),
        ConfigField::scalar(
            "ssh-public-key",
            "CRUSOE_SSH_PUBLIC_KEY",
            "crusoe.ssh_public_key",
        )
        .required(),
        ConfigField::scalar("image", "CRUSOE_IMAGE", "crusoe.image"),
    ],
    credential_fields: &["access_key_id", "secret_key"],
    offers: &[
        ("l40s-48gb.1x", "nvidia-l40s"),
        ("a100-80gb.1x", "nvidia-a100-80gb"),
        ("h100-80gb-sxm-ib.8x", "nvidia-h100-80gb"),
        ("h200-141gb-sxm-ib.8x", "nvidia-h200-141gb"),
    ],
    guest: GuestIdentity::InstanceName,
};

#[derive(Default)]
pub struct Api {
    client: reqwest::Client,
}

impl Api {
    pub fn new() -> Self {
        Self::default()
    }

    /// Send a signed request for `path` (from `/v1`) with `query` pairs.
    async fn call(
        &self,
        operation: &str,
        method: reqwest::Method,
        path: &str,
        query: &[(&str, String)],
        body: Option<&Value>,
    ) -> Result<Value, GpuCloudError> {
        let access_key = secret(VENDOR, "access_key_id").await?;
        let secret_key = secret(VENDOR, "secret_key").await?;
        let key = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(secret_key.trim_end_matches('='))
            .map_err(|error| {
                GpuCloudError::Credential(format!(
                    "Crusoe Cloud: field secret_key of Skarbiec role {} is not url-safe base64: {error}",
                    VENDOR.credential_role()
                ))
            })?;
        let mut sorted = query.to_vec();
        sorted.sort();
        let canonical = sorted
            .iter()
            .map(|(name, value)| format!("{name}={value}"))
            .collect::<Vec<_>>()
            .join("&");
        let timestamp = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        let payload = format!("{path}\n{canonical}\n{}\n{timestamp}\n", method.as_str());
        let signature = signing::base64_url(&signing::hmac_sha256(&key, payload.as_bytes()));
        let mut request = self
            .client
            .request(method, format!("{API}{path}"))
            .query(&sorted)
            .header("X-Crusoe-Timestamp", timestamp)
            .bearer_auth(format!("1.0:{access_key}:{signature}"));
        if let Some(body) = body {
            request = request.json(body);
        }
        http::exchange(VENDOR, operation, request).await
    }

    fn instances(&self) -> Result<String, GpuCloudError> {
        Ok(format!(
            "/v1/projects/{}/compute/vms/instances",
            required_setting(VENDOR, "project-id")?
        ))
    }
}

fn state(raw: &str) -> MachineState {
    match raw {
        "STATE_RUNNING" => MachineState::Running,
        "STATE_STOPPING" | "STATE_SHUTTING_DOWN" => MachineState::Stopping,
        "STATE_SHUTOFF" | "STATE_STOPPED" => MachineState::Stopped,
        "STATE_DELETING" => MachineState::Terminating,
        "STATE_DELETED" => MachineState::Terminated,
        "STATE_FAILED" | "STATE_ERROR" => MachineState::Failed,
        _ => MachineState::Provisioning,
    }
}

fn machine(operation: &str, instance: &Value) -> Result<Machine, GpuCloudError> {
    Ok(Machine {
        native_id: http::text(VENDOR, operation, instance, "/id")?,
        name: http::optional_text(instance, "/name").unwrap_or_default(),
        instance_type: http::text(VENDOR, operation, instance, "/type")?,
        state: state(&http::text(VENDOR, operation, instance, "/state")?),
        created_at: http::timestamp(instance, "/created_at"),
    })
}

#[async_trait]
impl GpuCloudApi for Api {
    async fn launch(&self, request: &LaunchRequest<'_>) -> Result<Machine, GpuCloudError> {
        let operation = format!("create VM {}", request.instance_type);
        let location = required_setting(VENDOR, "location")?;
        let capacities = self
            .call(
                &format!("read capacity of {} in {location}", request.instance_type),
                reqwest::Method::GET,
                "/v1/capacities",
                &[
                    ("location", location.clone()),
                    ("product_name", request.instance_type.to_string()),
                ],
                None,
            )
            .await?;
        let available = capacities
            .get("items")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|item| item.get("quantity").and_then(Value::as_i64))
            .any(i64::is_positive);
        if !available {
            return Err(GpuCloudError::capacity(
                VENDOR,
                request.instance_type,
                format!("location {location} reports {capacities}"),
            ));
        }
        let mut body = json!({
            "name": request.name,
            "type": request.instance_type,
            "location": location,
            "ssh_public_key": required_setting(VENDOR, "ssh-public-key")?,
            "startup_script": request.startup_script,
        });
        let image = setting(VENDOR, "image")?;
        if !image.is_empty() {
            body["image"] = json!(image);
        }
        let path = self.instances()?;
        self.call(&operation, reqwest::Method::POST, &path, &[], Some(&body))
            .await?;
        let listed = self
            .call(
                &format!("find VM {}", request.name),
                reqwest::Method::GET,
                &path,
                &[("names", request.name.to_string())],
                None,
            )
            .await?;
        let instance = listed.pointer("/items/0").ok_or_else(|| {
            GpuCloudError::response(
                VENDOR,
                &operation,
                format!(
                    "the create was accepted but no VM named {} is listed: {listed}",
                    request.name
                ),
            )
        })?;
        let mut launched = machine(&operation, instance)?;
        if launched.created_at.is_none() {
            launched.created_at = Some(http::launch_stamp().1);
        }
        Ok(launched)
    }

    async fn terminate(&self, native_id: &str) -> Result<(), GpuCloudError> {
        self.call(
            &format!("delete VM {native_id}"),
            reqwest::Method::DELETE,
            &format!("{}/{native_id}", self.instances()?),
            &[],
            None,
        )
        .await
        .map(drop)
    }

    async fn machine(&self, native_id: &str) -> Result<Option<Machine>, GpuCloudError> {
        let operation = format!("read VM {native_id}");
        match self
            .call(
                &operation,
                reqwest::Method::GET,
                &format!("{}/{native_id}", self.instances()?),
                &[],
                None,
            )
            .await
        {
            Ok(instance) => machine(&operation, &instance).map(Some),
            Err(GpuCloudError::NotFound { .. }) => Ok(None),
            Err(error) => Err(error),
        }
    }

    async fn machines(&self) -> Result<Vec<Machine>, GpuCloudError> {
        let operation = "list VMs";
        let answer = self
            .call(
                operation,
                reqwest::Method::GET,
                &self.instances()?,
                &[],
                None,
            )
            .await?;
        answer
            .get("items")
            .and_then(Value::as_array)
            .ok_or_else(|| {
                GpuCloudError::response(VENDOR, operation, format!("no items in {answer}"))
            })?
            .iter()
            .map(|instance| machine(operation, instance))
            .collect()
    }
}
