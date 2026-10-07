//! Scaleway GPU Instances.
//!
//! API: <https://www.scaleway.com/en/developers/api/instance/>. The secret
//! key travels in `X-Auth-Token`. A server is created stopped (`POST
//! /instance/v1/zones/{zone}/servers` with `name`, `commercial_type`,
//! `image`, `project`, `tags`), receives its cloud-init through `PATCH
//! .../servers/{id}/user_data/cloud-init`, and starts with the `poweron`
//! action; the `terminate` action releases it with its volumes. `GET
//! .../servers/{id}` and the paged `GET .../servers` read them. A refusal of
//! type `out_of_stock` is capacity.

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::capabilities::{ConfigField, GpuCloudVendor};
use crate::providers::gpu_cloud::access::{required_setting, secret};
use crate::providers::gpu_cloud::api::{
    GpuCloudApi, GpuCloudError, LaunchRequest, Machine, MachineState,
};
use crate::providers::gpu_cloud::http;
use crate::providers::gpu_cloud::{GuestIdentity, VendorProfile};

const VENDOR: GpuCloudVendor = GpuCloudVendor::Scaleway;
const API: &str = "https://api.scaleway.com/instance/v1/zones";

pub const PROFILE: VendorProfile = VendorProfile {
    vendor: VENDOR,
    summary: "Rent Scaleway GPU Instances and run an agent on each.",
    config: &[
        ConfigField::scalar("zone", "SCALEWAY_ZONE", "scaleway.zone").required(),
        ConfigField::scalar("project-id", "SCALEWAY_PROJECT_ID", "scaleway.project_id").required(),
        ConfigField::scalar("image", "SCALEWAY_IMAGE", "scaleway.image").required(),
    ],
    credential_fields: &["secret_key"],
    offers: &[
        ("L4-1-24G", "nvidia-l4"),
        ("L40S-1-48G", "nvidia-l40s"),
        ("H100-1-80G", "nvidia-h100-80gb"),
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

    async fn request(
        &self,
        method: reqwest::Method,
        path: &str,
    ) -> Result<reqwest::RequestBuilder, GpuCloudError> {
        let zone = required_setting(VENDOR, "zone")?;
        let key = secret(VENDOR, "secret_key").await?;
        Ok(self
            .client
            .request(method, format!("{API}/{zone}{path}"))
            .header("X-Auth-Token", key))
    }

    async fn call(
        &self,
        operation: &str,
        method: reqwest::Method,
        path: &str,
        body: Option<&Value>,
    ) -> Result<Value, GpuCloudError> {
        let mut request = self.request(method, path).await?;
        if let Some(body) = body {
            request = request.json(body);
        }
        http::exchange(VENDOR, operation, request).await
    }

    async fn action(&self, native_id: &str, action: &str) -> Result<(), GpuCloudError> {
        self.call(
            &format!("{action} server {native_id}"),
            reqwest::Method::POST,
            &format!("/servers/{native_id}/action"),
            Some(&json!({ "action": action })),
        )
        .await
        .map(drop)
    }
}

fn state(raw: &str) -> MachineState {
    match raw {
        "running" => MachineState::Running,
        "stopping" => MachineState::Stopping,
        "stopped" | "stopped in place" => MachineState::Stopped,
        "locked" => MachineState::Failed,
        _ => MachineState::Provisioning,
    }
}

fn machine(operation: &str, server: &Value) -> Result<Machine, GpuCloudError> {
    Ok(Machine {
        native_id: http::text(VENDOR, operation, server, "/id")?,
        name: http::optional_text(server, "/name").unwrap_or_default(),
        instance_type: http::text(VENDOR, operation, server, "/commercial_type")?,
        state: state(&http::text(VENDOR, operation, server, "/state")?),
        created_at: http::timestamp(server, "/creation_date"),
    })
}

#[async_trait]
impl GpuCloudApi for Api {
    async fn launch(&self, request: &LaunchRequest<'_>) -> Result<Machine, GpuCloudError> {
        let operation = format!("create server {}", request.instance_type);
        let body = json!({
            "name": request.name,
            "commercial_type": request.instance_type,
            "image": required_setting(VENDOR, "image")?,
            "project": required_setting(VENDOR, "project-id")?,
            "tags": ["stado-agent"],
        });
        let answer = match self
            .call(&operation, reqwest::Method::POST, "/servers", Some(&body))
            .await
        {
            Ok(answer) => answer,
            Err(error) if http::refusal_member(&error, "/type").as_deref() == Some("out_of_stock") => {
                return Err(GpuCloudError::capacity(
                    VENDOR,
                    request.instance_type,
                    error.detail().to_string(),
                ))
            }
            Err(error) => return Err(error),
        };
        let server = answer.get("server").ok_or_else(|| {
            GpuCloudError::response(VENDOR, &operation, format!("no server in {answer}"))
        })?;
        let launched = machine(&operation, server)?;
        let user_data = self
            .request(
                reqwest::Method::PATCH,
                &format!("/servers/{}/user_data/cloud-init", launched.native_id),
            )
            .await?
            .header(reqwest::header::CONTENT_TYPE, "text/plain")
            .body(request.startup_script.to_string());
        http::exchange_text(
            VENDOR,
            &format!("write cloud-init of server {}", launched.native_id),
            user_data,
        )
        .await?;
        self.action(&launched.native_id, "poweron").await?;
        Ok(launched)
    }

    async fn terminate(&self, native_id: &str) -> Result<(), GpuCloudError> {
        self.action(native_id, "terminate").await
    }

    async fn machine(&self, native_id: &str) -> Result<Option<Machine>, GpuCloudError> {
        let operation = format!("read server {native_id}");
        match self
            .call(&operation, reqwest::Method::GET, &format!("/servers/{native_id}"), None)
            .await
        {
            Ok(answer) => {
                let server = answer.get("server").ok_or_else(|| {
                    GpuCloudError::response(VENDOR, &operation, format!("no server in {answer}"))
                })?;
                machine(&operation, server).map(Some)
            }
            Err(GpuCloudError::NotFound { .. }) => Ok(None),
            Err(error) => Err(error),
        }
    }

    async fn machines(&self) -> Result<Vec<Machine>, GpuCloudError> {
        let operation = "list servers";
        let mut machines = Vec::new();
        let mut page = std::num::NonZeroU32::MIN.get();
        loop {
            let answer = self
                .call(operation, reqwest::Method::GET, &format!("/servers?page={page}"), None)
                .await?;
            let servers = answer.get("servers").and_then(Value::as_array).ok_or_else(|| {
                GpuCloudError::response(VENDOR, operation, format!("no servers in {answer}"))
            })?;
            if servers.is_empty() {
                return Ok(machines);
            }
            for server in servers {
                machines.push(machine(operation, server)?);
            }
            page += std::num::NonZeroU32::MIN.get();
        }
    }
}
